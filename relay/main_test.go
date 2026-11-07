package main

import (
	"context"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/fiatjaf/eventstore/badger"
	"github.com/fiatjaf/khatru"
	"github.com/nbd-wtf/go-nostr"
)

// buildTestRelay wires up the same policy as main(), against a throwaway
// badger dir, and serves it over httptest so each test gets an isolated
// relay instance and a real websocket round trip (not an in-process shortcut).
func buildTestRelay(t *testing.T) (*khatru.Relay, string) {
	t.Helper()
	db := &badger.BadgerBackend{Path: t.TempDir()}
	if err := db.Init(); err != nil {
		t.Fatalf("db init: %v", err)
	}
	t.Cleanup(db.Close)

	relay := khatru.NewRelay()
	relay.StoreEvent = append(relay.StoreEvent, db.SaveEvent)
	relay.QueryEvents = append(relay.QueryEvents, db.QueryEvents)
	relay.DeleteEvent = append(relay.DeleteEvent, db.DeleteEvent)
	relay.ReplaceEvent = append(relay.ReplaceEvent, db.ReplaceEvent)
	relay.CountEvents = append(relay.CountEvents, db.CountEvents)
	relay.RejectEvent = append(relay.RejectEvent, func(_ context.Context, event *nostr.Event) (bool, string) {
		if !isKindAllowed(event.Kind) {
			return true, "blocked: kind not allowed"
		}
		return false, ""
	})

	srv := httptest.NewServer(relay)
	t.Cleanup(srv.Close)
	wsURL := "ws" + strings.TrimPrefix(srv.URL, "http")
	return relay, wsURL
}

func TestAllowedKindIsStoredAndQueryable(t *testing.T) {
	_, wsURL := buildTestRelay(t)
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	conn, err := nostr.RelayConnect(ctx, wsURL)
	if err != nil {
		t.Fatalf("connect: %v", err)
	}
	defer conn.Close()

	sk := nostr.GeneratePrivateKey()
	pk, _ := nostr.GetPublicKey(sk)
	evt := nostr.Event{
		PubKey:    pk,
		CreatedAt: nostr.Now(),
		Kind:      30078, // NIP-78, on the allowlist
		Tags:      nostr.Tags{{"d", "evermore:test"}},
		Content:   `{"theme":"dark"}`,
	}
	if err := evt.Sign(sk); err != nil {
		t.Fatalf("sign: %v", err)
	}

	if err := conn.Publish(ctx, evt); err != nil {
		t.Fatalf("publish of allowed kind was rejected: %v", err)
	}

	sub, err := conn.Subscribe(ctx, nostr.Filters{{Kinds: []int{30078}, Authors: []string{pk}}})
	if err != nil {
		t.Fatalf("subscribe: %v", err)
	}
	select {
	case got := <-sub.Events:
		if got.ID != evt.ID {
			t.Fatalf("got event %s, want %s", got.ID, evt.ID)
		}
	case <-time.After(3 * time.Second):
		t.Fatal("timed out waiting for the event we just published to come back")
	}
}

func TestDisallowedKindIsRejected(t *testing.T) {
	_, wsURL := buildTestRelay(t)
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	conn, err := nostr.RelayConnect(ctx, wsURL)
	if err != nil {
		t.Fatalf("connect: %v", err)
	}
	defer conn.Close()

	sk := nostr.GeneratePrivateKey()
	pk, _ := nostr.GetPublicKey(sk)
	evt := nostr.Event{
		PubKey:    pk,
		CreatedAt: nostr.Now(),
		Kind:      9999, // not on the allowlist
		Content:   "should be rejected",
	}
	if err := evt.Sign(sk); err != nil {
		t.Fatalf("sign: %v", err)
	}

	err = conn.Publish(ctx, evt)
	if err == nil {
		t.Fatal("expected publish of a non-allowlisted kind to be rejected, but it succeeded")
	}
}
