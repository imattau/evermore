//! Writable FUSE view backed by the same cached directory and sync path as
//! the `~/Nostr` watcher mode.

use crate::{App, ManifestEntry, rescan, valid_filename};
use fuser::{
    FileAttr, FileType, Filesystem, MountOption, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory,
    ReplyEntry, ReplyOpen, ReplyWrite, Request,
};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

const ROOT: u64 = 1;
const TTL: Duration = Duration::from_secs(1);

pub fn mount(app: App, mountpoint: &Path) -> Result<(), String> {
    fs::create_dir_all(mountpoint).map_err(|e| format!("creating mount point: {e}"))?;
    let filesystem = BlossomFs::new(app);
    fuser::mount2(
        filesystem,
        mountpoint,
        &[
            MountOption::FSName("evermore-blossom".into()),
            MountOption::RW,
            MountOption::DefaultPermissions,
        ],
    )
    .map_err(|e| format!("mounting {}: {e}", mountpoint.display()))
}

struct BlossomFs {
    app: App,
    inode_by_name: HashMap<String, u64>,
    name_by_inode: HashMap<u64, String>,
    next_inode: u64,
    handles: HashMap<u64, String>,
    next_handle: u64,
}

impl BlossomFs {
    fn new(app: App) -> Self {
        Self {
            app,
            inode_by_name: HashMap::new(),
            name_by_inode: HashMap::new(),
            next_inode: 2,
            handles: HashMap::new(),
            next_handle: 1,
        }
    }

    fn inode(&mut self, name: &str) -> u64 {
        if let Some(ino) = self.inode_by_name.get(name) {
            return *ino;
        }
        let ino = self.next_inode;
        self.next_inode += 1;
        self.inode_by_name.insert(name.to_string(), ino);
        self.name_by_inode.insert(ino, name.to_string());
        ino
    }

    fn attr(&self, ino: u64) -> Option<FileAttr> {
        let (kind, size, perm, nlink) = if ino == ROOT {
            (FileType::Directory, 0, 0o755, 2)
        } else {
            let name = self.name_by_inode.get(&ino)?;
            let meta = fs::metadata(self.app.shared.dir.join(name)).ok()?;
            (FileType::RegularFile, meta.len(), 0o644, 1)
        };
        let now = SystemTime::now();
        Some(FileAttr {
            ino,
            size,
            blocks: size.div_ceil(512),
            atime: now,
            mtime: now,
            ctime: now,
            crtime: now,
            kind,
            perm,
            nlink,
            uid: unsafe { libc::getuid() },
            gid: unsafe { libc::getgid() },
            rdev: 0,
            blksize: 4096,
            flags: 0,
        })
    }

    fn filename(&self, ino: u64) -> Option<&str> {
        self.name_by_inode.get(&ino).map(String::as_str)
    }

    fn handle_for(&mut self, filename: &str) -> u64 {
        let fh = self.next_handle;
        self.next_handle += 1;
        self.handles.insert(fh, filename.to_string());
        fh
    }

    fn sync(&self) {
        rescan(&self.app);
    }
}

impl Filesystem for BlossomFs {
    fn lookup(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEntry) {
        if parent != ROOT {
            reply.error(libc::ENOTDIR);
            return;
        }
        let name = match name.to_str() {
            Some(name) if valid_filename(name) => name,
            _ => {
                reply.error(libc::ENOENT);
                return;
            }
        };
        let ino = self.inode(name);
        match self.attr(ino) {
            Some(attr) => reply.entry(&TTL, &attr, 0),
            None => reply.error(libc::ENOENT),
        }
    }

    fn getattr(&mut self, _req: &Request<'_>, ino: u64, _fh: Option<u64>, reply: ReplyAttr) {
        match self.attr(ino) {
            Some(attr) => reply.attr(&TTL, &attr),
            None => reply.error(libc::ENOENT),
        }
    }

    fn setattr(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        mode: Option<u32>,
        uid: Option<u32>,
        gid: Option<u32>,
        size: Option<u64>,
        _atime: Option<fuser::TimeOrNow>,
        _mtime: Option<fuser::TimeOrNow>,
        _ctime: Option<SystemTime>,
        _fh: Option<u64>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<u32>,
        reply: ReplyAttr,
    ) {
        let Some(name) = self.filename(ino).map(str::to_string) else {
            reply.error(libc::EISDIR);
            return;
        };
        let _manifest = self.app.shared.manifest.lock().unwrap();
        if uid.is_some() || gid.is_some() {
            reply.error(libc::EPERM);
            return;
        }
        let path = self.app.shared.dir.join(name);
        if let Some(size) = size {
            let result = fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .and_then(|file| file.set_len(size));
            if result.is_err() {
                reply.error(libc::EIO);
                return;
            }
        }
        if let Some(mode) = mode {
            use std::os::unix::fs::PermissionsExt;
            if fs::set_permissions(&path, fs::Permissions::from_mode(mode & 0o777)).is_err() {
                reply.error(libc::EIO);
                return;
            }
        }
        match self.attr(ino) {
            Some(attr) => reply.attr(&TTL, &attr),
            None => reply.error(libc::ENOENT),
        }
    }

    fn readdir(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        if ino != ROOT {
            reply.error(libc::ENOTDIR);
            return;
        }
        let mut rows = vec![
            (ROOT, ".".to_string(), FileType::Directory),
            (ROOT, "..".to_string(), FileType::Directory),
        ];
        if let Ok(entries) = fs::read_dir(&self.app.shared.dir) {
            for entry in entries.flatten() {
                if !entry.path().is_file() {
                    continue;
                }
                let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                    continue;
                };
                let ino = self.inode(&name);
                rows.push((ino, name, FileType::RegularFile));
            }
        }
        for (index, (child_ino, name, kind)) in
            rows.into_iter().enumerate().skip(offset.max(0) as usize)
        {
            if reply.add(child_ino, (index + 1) as i64, kind, name) {
                break;
            }
        }
        reply.ok();
    }

    fn open(&mut self, _req: &Request<'_>, ino: u64, flags: i32, reply: ReplyOpen) {
        let Some(name) = self.filename(ino).map(str::to_string) else {
            reply.error(libc::EISDIR);
            return;
        };
        let path = self.app.shared.dir.join(&name);
        let truncate_result = if flags & libc::O_TRUNC != 0 {
            let _manifest = self.app.shared.manifest.lock().unwrap();
            fs::write(&path, [])
        } else {
            Ok(())
        };
        if truncate_result.is_err() {
            reply.error(libc::EIO);
            return;
        }
        let fh = self.handle_for(&name);
        reply.opened(fh, 0);
    }

    fn read(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyData,
    ) {
        let Some(name) = self.filename(ino).map(str::to_string) else {
            reply.error(libc::ENOENT);
            return;
        };
        let manifest = self.app.shared.manifest.lock().unwrap();
        let path = self.app.shared.dir.join(&name);
        if !path.exists() {
            let entry = manifest.get(&name).cloned();
            if let Some(ManifestEntry { sha256, .. }) = entry {
                let client = crate::blossom_client::BlossomClient::new(
                    &self.app.shared.blossom_url,
                    &self.app.shared.signer,
                );
                match client.download(&sha256).and_then(|bytes| {
                    if crate::blossom_client::hex_sha256(&bytes) != sha256 {
                        return Err("downloaded blob failed its SHA-256 check".into());
                    }
                    fs::write(&path, bytes).map_err(|e| e.to_string())
                }) {
                    Ok(()) => {}
                    Err(_) => {
                        reply.error(libc::EIO);
                        return;
                    }
                }
            }
        }
        match fs::read(path) {
            Ok(bytes) => {
                let start = offset.max(0) as usize;
                if start >= bytes.len() {
                    reply.data(&[]);
                } else {
                    reply.data(&bytes[start..bytes.len().min(start.saturating_add(size as usize))]);
                }
            }
            Err(_) => reply.error(libc::ENOENT),
        }
    }

    fn write(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        data: &[u8],
        _write_flags: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyWrite,
    ) {
        let Some(name) = self.filename(ino).map(str::to_string) else {
            reply.error(libc::ENOENT);
            return;
        };
        let _manifest = self.app.shared.manifest.lock().unwrap();
        if offset < 0 {
            reply.error(libc::EINVAL);
            return;
        }
        let path = self.app.shared.dir.join(name);
        let mut bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(_) => {
                reply.error(libc::EIO);
                return;
            }
        };
        let start = offset as usize;
        if bytes.len() < start {
            bytes.resize(start, 0);
        }
        let end = start.saturating_add(data.len());
        if end > bytes.len() {
            bytes.resize(end, 0);
        }
        bytes[start..end].copy_from_slice(data);
        match fs::write(path, bytes) {
            Ok(()) => reply.written(data.len() as u32),
            Err(_) => reply.error(libc::EIO),
        }
    }

    fn flush(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _fh: u64,
        _lock_owner: u64,
        reply: fuser::ReplyEmpty,
    ) {
        // FUSE may issue flush more than once for dup/fd-close paths. Sync on
        // release (once per handle) and explicit fsync instead, so an editor
        // never publishes an intermediate empty or partial buffer.
        reply.ok();
    }

    fn fsync(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _fh: u64,
        _datasync: bool,
        reply: fuser::ReplyEmpty,
    ) {
        self.sync();
        reply.ok();
    }

    fn release(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        fh: u64,
        _flags: i32,
        _lock_owner: Option<u64>,
        _flush: bool,
        reply: fuser::ReplyEmpty,
    ) {
        self.handles.remove(&fh);
        self.sync();
        reply.ok();
    }

    fn create(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        _flags: i32,
        reply: ReplyCreate,
    ) {
        if parent != ROOT {
            reply.error(libc::ENOTDIR);
            return;
        }
        let Some(name) = name
            .to_str()
            .filter(|name| valid_filename(name))
            .map(str::to_string)
        else {
            reply.error(libc::EINVAL);
            return;
        };
        let path = self.app.shared.dir.join(&name);
        let create_result = {
            let _manifest = self.app.shared.manifest.lock().unwrap();
            if path.exists() {
                Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists))
            } else {
                fs::write(&path, [])
            }
        };
        if create_result.is_err() {
            reply.error(libc::EIO);
            return;
        }
        let ino = self.inode(&name);
        let Some(attr) = self.attr(ino) else {
            reply.error(libc::EIO);
            return;
        };
        let fh = self.handle_for(&name);
        reply.created(&TTL, &attr, 0, fh, 0);
    }

    fn unlink(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: fuser::ReplyEmpty) {
        if parent != ROOT {
            reply.error(libc::ENOTDIR);
            return;
        }
        let Some(name) = name.to_str().filter(|name| valid_filename(name)) else {
            reply.error(libc::EINVAL);
            return;
        };
        let result = {
            let _manifest = self.app.shared.manifest.lock().unwrap();
            fs::remove_file(self.app.shared.dir.join(name))
        };
        match result {
            Ok(()) => {
                self.sync();
                reply.ok();
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => reply.error(libc::ENOENT),
            Err(_) => reply.error(libc::EIO),
        }
    }

    fn rename(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &OsStr,
        newparent: u64,
        newname: &OsStr,
        _flags: u32,
        reply: fuser::ReplyEmpty,
    ) {
        let (Some(old), Some(new)) = (
            name.to_str().filter(|n| valid_filename(n)),
            newname.to_str().filter(|n| valid_filename(n)),
        ) else {
            reply.error(libc::EINVAL);
            return;
        };
        if parent != ROOT || newparent != ROOT {
            reply.error(libc::EXDEV);
            return;
        }
        let result = {
            let _manifest = self.app.shared.manifest.lock().unwrap();
            fs::rename(self.app.shared.dir.join(old), self.app.shared.dir.join(new))
        };
        match result {
            Ok(()) => {
                if let Some(ino) = self.inode_by_name.remove(old) {
                    self.name_by_inode.remove(&ino);
                }
                if let Some(ino) = self.inode_by_name.remove(new) {
                    self.name_by_inode.remove(&ino);
                }
                self.sync();
                reply.ok();
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => reply.error(libc::ENOENT),
            Err(_) => reply.error(libc::EIO),
        }
    }
}
