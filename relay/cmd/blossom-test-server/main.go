// blossom-test-server is a minimal local Blossom (BUD-01/02) server used
// ONLY to test evermore-blossom-fs against -- it is not part of Evermore's
// shipped architecture. Per PROJECT_IDEA.md's own system-concerns table,
// Blossom servers are external infrastructure a user points Evermore at,
// not something Evermore provides; this exists purely so Phase 4 has
// something local and controllable to develop and test against instead of
// hitting a real public Blossom server from dev/CI.
//
// Implements just enough of BUD-01 (server requirements) and BUD-02
// (upload/list/delete) to exercise evermore-blossom-fs's client: PUT
// /upload, GET /<sha256>, GET /list/<pubkey>, DELETE /<sha256>, all
// NIP-98 (BUD-01 auth) authenticated.
package main

import (
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"log"
	"net/http"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/nbd-wtf/go-nostr"
)

const defaultPort = "4870"
const authEventKind = 27235 // NIP-98 HTTP Auth
const authWindow = 60 * time.Second

type blobDescriptor struct {
	URL      string `json:"url"`
	SHA256   string `json:"sha256"`
	Size     int64  `json:"size"`
	Type     string `json:"type,omitempty"`
	Uploaded int64  `json:"uploaded"`
}

type server struct {
	storageDir string
	baseURL    string

	mu   sync.RWMutex
	meta map[string]blobDescriptor // sha256 -> descriptor
	// sha256 -> owner pubkey (hex), kept separate from the descriptor JSON
	// we return to clients (owner isn't part of the public blob shape).
	owner map[string]string
}

func main() {
	port := os.Getenv("BLOSSOM_PORT")
	if port == "" {
		port = defaultPort
	}
	storageDir := os.Getenv("BLOSSOM_STORAGE_DIR")
	if storageDir == "" {
		storageDir = filepath.Join(os.TempDir(), "evermore-blossom-test")
	}
	if err := os.MkdirAll(storageDir, 0o700); err != nil {
		log.Fatalf("creating storage dir: %v", err)
	}

	s := &server{
		storageDir: storageDir,
		baseURL:    "http://127.0.0.1:" + port,
		meta:       map[string]blobDescriptor{},
		owner:      map[string]string{},
	}

	mux := http.NewServeMux()
	mux.HandleFunc("/upload", s.handleUpload)
	mux.HandleFunc("/list/", s.handleList)
	mux.HandleFunc("/", s.handleBlob)

	log.Printf("blossom-test-server listening on %s (storage: %s)", s.baseURL, storageDir)
	log.Fatal(http.ListenAndServe("127.0.0.1:"+port, mux))
}

func (s *server) handleUpload(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPut {
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
		return
	}
	body, err := io.ReadAll(r.Body)
	if err != nil {
		http.Error(w, "couldn't read body", http.StatusBadRequest)
		return
	}
	sum := sha256.Sum256(body)
	sha := hex.EncodeToString(sum[:])

	pubkey, authErr := verifyAuth(r, "PUT", &sha)
	if authErr != "" {
		http.Error(w, authErr, http.StatusUnauthorized)
		return
	}

	if err := os.WriteFile(filepath.Join(s.storageDir, sha), body, 0o600); err != nil {
		http.Error(w, fmt.Sprintf("couldn't store blob: %v", err), http.StatusInternalServerError)
		return
	}

	desc := blobDescriptor{
		URL:      s.baseURL + "/" + sha,
		SHA256:   sha,
		Size:     int64(len(body)),
		Type:     r.Header.Get("Content-Type"),
		Uploaded: time.Now().Unix(),
	}
	s.mu.Lock()
	s.meta[sha] = desc
	s.owner[sha] = pubkey
	s.mu.Unlock()

	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(desc)
}

func (s *server) handleBlob(w http.ResponseWriter, r *http.Request) {
	sha := strings.TrimPrefix(r.URL.Path, "/")
	sha = strings.TrimSuffix(sha, filepath.Ext(sha)) // tolerate a .ext suffix some clients add
	if sha == "" {
		http.NotFound(w, r)
		return
	}

	switch r.Method {
	case http.MethodGet, http.MethodHead:
		path := filepath.Join(s.storageDir, sha)
		data, err := os.ReadFile(path)
		if err != nil {
			http.NotFound(w, r)
			return
		}
		s.mu.RLock()
		desc, ok := s.meta[sha]
		s.mu.RUnlock()
		if ok && desc.Type != "" {
			w.Header().Set("Content-Type", desc.Type)
		}
		if r.Method == http.MethodHead {
			w.Header().Set("Content-Length", strconv.Itoa(len(data)))
			return
		}
		_, _ = w.Write(data)

	case http.MethodDelete:
		pubkey, authErr := verifyAuth(r, "DELETE", &sha)
		if authErr != "" {
			http.Error(w, authErr, http.StatusUnauthorized)
			return
		}
		s.mu.Lock()
		owner, exists := s.owner[sha]
		if exists && owner == pubkey {
			delete(s.meta, sha)
			delete(s.owner, sha)
		}
		s.mu.Unlock()
		if !exists {
			http.NotFound(w, r)
			return
		}
		if owner != pubkey {
			http.Error(w, "not the owner", http.StatusForbidden)
			return
		}
		_ = os.Remove(filepath.Join(s.storageDir, sha))
		w.WriteHeader(http.StatusOK)

	default:
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
	}
}

func (s *server) handleList(w http.ResponseWriter, r *http.Request) {
	pubkey := strings.TrimPrefix(r.URL.Path, "/list/")
	if pubkey == "" {
		http.Error(w, "missing pubkey", http.StatusBadRequest)
		return
	}
	if _, authErr := verifyAuth(r, "GET", nil); authErr != "" {
		http.Error(w, authErr, http.StatusUnauthorized)
		return
	}

	s.mu.RLock()
	defer s.mu.RUnlock()
	descs := make([]blobDescriptor, 0)
	for sha, owner := range s.owner {
		if owner == pubkey {
			descs = append(descs, s.meta[sha])
		}
	}
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(descs)
}

// verifyAuth checks a NIP-98 (BUD-01) Authorization header and returns the
// authenticated pubkey (hex). If expectedPayloadSHA256 is non-nil, the
// event's "payload" tag must match it (required for uploads).
func verifyAuth(r *http.Request, expectedMethod string, expectedPayloadSHA256 *string) (pubkey string, errMsg string) {
	header := r.Header.Get("Authorization")
	const prefix = "Nostr "
	if !strings.HasPrefix(header, prefix) {
		return "", "missing Nostr Authorization header"
	}
	raw, err := base64.StdEncoding.DecodeString(strings.TrimPrefix(header, prefix))
	if err != nil {
		return "", "invalid base64 in Authorization header"
	}

	var evt nostr.Event
	if err := json.Unmarshal(raw, &evt); err != nil {
		return "", "invalid event JSON in Authorization header"
	}
	if ok, err := evt.CheckSignature(); err != nil || !ok {
		return "", "invalid event signature"
	}
	if evt.Kind != authEventKind {
		return "", "wrong event kind for NIP-98 auth"
	}
	if time.Since(evt.CreatedAt.Time()).Abs() > authWindow {
		return "", "auth event timestamp out of window"
	}

	method := evt.Tags.GetFirst([]string{"method"})
	if method == nil || (*method)[1] != expectedMethod {
		return "", "auth event method tag doesn't match request"
	}
	urlTag := evt.Tags.GetFirst([]string{"u"})
	if urlTag == nil {
		return "", "auth event missing u tag"
	}
	if expectedPayloadSHA256 != nil {
		payloadTag := evt.Tags.GetFirst([]string{"payload"})
		if payloadTag == nil || (*payloadTag)[1] != *expectedPayloadSHA256 {
			return "", "auth event payload tag doesn't match uploaded content"
		}
	}

	return evt.PubKey, ""
}
