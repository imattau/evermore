// evermore-relay-supervisor: the local control-plane relay described in
// PROJECT_IDEA.md's "The Local Control-Plane Relay" section. Wraps khatru
// (https://github.com/fiatjaf/khatru) with Evermore's policy: loopback-only
// access and a kind allowlist, both administered via NIP-86 so there's no
// proprietary relay-management API layered on top.
package main

import (
	"context"
	"log"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"sync"

	"github.com/fiatjaf/eventstore/badger"
	"github.com/fiatjaf/khatru"
	"github.com/nbd-wtf/go-nostr"
	"github.com/nbd-wtf/go-nostr/nip86"
)

const defaultPort = "4869"

// Primitive-first kind allowlist (see PROJECT_IDEA.md): only kinds that
// have an actual Evermore use today are allowed. New kinds get added here
// (or via the NIP-86 allow_kind call, which edits the same map) as new
// daemons start using the relay -- not pre-emptively.
var allowedKinds = struct {
	sync.RWMutex
	set map[int]bool
}{set: map[int]bool{
	0:     true, // profile metadata
	1:     true, // text note -- also doubles as the local notify event (Phase 3: ["t","evermore-notify"])
	3:     true, // contacts/follows
	5:     true, // deletion request
	1059:  true, // NIP-17/59 gift wrap (DMs, notifications)
	10002: true, // NIP-65 relay list
	30078: true, // NIP-78 app/settings data
	10000: true, // NIP-51 mute list
	10006: true, // NIP-51 blocked relays
	30000: true, // NIP-51 categorized people list
	30002: true, // NIP-51 categorized relay list
	30301: true, // Evermore file index (Phase 4) -- custom kind: no existing NIP
	// covers a per-directory addressable manifest of {filename: {sha256, size}}
	32267: true, // Software application (Phase 5 npack) -- package metadata
	30063: true, // Release artifact set (Phase 5 npack) -- version pointer + Blossom URL/hash
	31922: true, // NIP-52 date-based calendar events
	31923: true, // NIP-52 time-based calendar events
	31924: true, // NIP-52 calendars
	31925: true, // NIP-52 RSVPs
}}

func isKindAllowed(kind int) bool {
	allowedKinds.RLock()
	defer allowedKinds.RUnlock()
	return allowedKinds.set[kind]
}

func main() {
	port := os.Getenv("EVERMORE_RELAY_PORT")
	if port == "" {
		port = defaultPort
	}

	dataDir := relayDataDir()
	if err := os.MkdirAll(dataDir, 0o700); err != nil {
		log.Fatalf("creating data dir %s: %v", dataDir, err)
	}

	db := &badger.BadgerBackend{Path: filepath.Join(dataDir, "events")}
	if err := db.Init(); err != nil {
		log.Fatalf("initializing event store: %v", err)
	}
	defer db.Close()

	relay := khatru.NewRelay()
	relay.Info.Name = "evermore-control-plane"
	relay.Info.Description = "Evermore's local control-plane relay -- settings, notifications, and trust events for daemons on this machine. Not a public relay."
	relay.Info.Software = "https://github.com/imattau/evermore"
	relay.Info.Version = "0.1.0"

	relay.StoreEvent = append(relay.StoreEvent, db.SaveEvent)
	relay.QueryEvents = append(relay.QueryEvents, db.QueryEvents)
	relay.DeleteEvent = append(relay.DeleteEvent, db.DeleteEvent)
	relay.ReplaceEvent = append(relay.ReplaceEvent, db.ReplaceEvent)
	relay.CountEvents = append(relay.CountEvents, db.CountEvents)

	// Loopback-only: this relay is the internal bus, never meant to be
	// reachable from the network. Binding to 127.0.0.1 below is the primary
	// control; this is defense in depth against e.g. a misconfigured proxy.
	relay.RejectConnection = append(relay.RejectConnection, func(r *http.Request) bool {
		host, _, err := net.SplitHostPort(r.RemoteAddr)
		if err != nil {
			host = r.RemoteAddr
		}
		ip := net.ParseIP(host)
		return ip == nil || !ip.IsLoopback()
	})

	relay.RejectEvent = append(relay.RejectEvent, func(_ context.Context, event *nostr.Event) (bool, string) {
		if !isKindAllowed(event.Kind) {
			return true, "blocked: kind " + strconv.Itoa(event.Kind) + " is not on the control-plane relay's allowlist"
		}
		return false, ""
	})

	relay.ManagementAPI.AllowKind = func(_ context.Context, kind int) error {
		allowedKinds.Lock()
		allowedKinds.set[kind] = true
		allowedKinds.Unlock()
		return nil
	}
	relay.ManagementAPI.DisallowKind = func(_ context.Context, kind int) error {
		allowedKinds.Lock()
		delete(allowedKinds.set, kind)
		allowedKinds.Unlock()
		return nil
	}
	relay.ManagementAPI.ListAllowedKinds = func(_ context.Context) ([]int, error) {
		allowedKinds.RLock()
		defer allowedKinds.RUnlock()
		kinds := make([]int, 0, len(allowedKinds.set))
		for k := range allowedKinds.set {
			kinds = append(kinds, k)
		}
		sort.Ints(kinds)
		return kinds, nil
	}
	relay.ManagementAPI.Stats = func(ctx context.Context) (nip86.Response, error) {
		count, err := db.CountEvents(ctx, nostr.Filter{})
		if err != nil {
			return nip86.Response{}, err
		}
		return nip86.Response{Result: map[string]any{"stored_events": count}}, nil
	}

	addr := "127.0.0.1:" + port
	log.Printf("evermore-relay-supervisor listening on ws://%s (data: %s)", addr, dataDir)
	if err := http.ListenAndServe(addr, relay); err != nil {
		log.Fatal(err)
	}
}

func relayDataDir() string {
	if dir := os.Getenv("EVERMORE_RELAY_DATA_DIR"); dir != "" {
		return dir
	}
	base := os.Getenv("XDG_DATA_HOME")
	if base == "" {
		home, err := os.UserHomeDir()
		if err != nil {
			home = "."
		}
		base = filepath.Join(home, ".local", "share")
	}
	return filepath.Join(base, "evermore", "relay")
}
