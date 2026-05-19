// Package substrate is a thin WebSocket client for the bridgechain node.
//
// It speaks the standard Substrate JSON-RPC, including the BEEFY-specific
// subscription `beefy_subscribeJustifications`. Decoding of the SCALE-
// encoded SignedCommitment lives in the `beefy` package, not here — this
// package only deals with transport.
package substrate

import (
	"context"
	"encoding/json"
	"fmt"
	"log/slog"
	"sync"
	"sync/atomic"

	"github.com/gorilla/websocket"
)

// Client is a JSON-RPC 2.0 client over WebSocket. Supports both
// request/response calls and subscriptions (each subscription gets its own
// read goroutine fan-in).
type Client struct {
	ws *websocket.Conn

	mu        sync.Mutex
	nextID    atomic.Uint64
	pending   map[uint64]chan json.RawMessage
	subs      map[string]chan json.RawMessage
	closeOnce sync.Once
	closed    chan struct{}
}

// Dial opens a WebSocket connection to the node's RPC endpoint.
func Dial(ctx context.Context, url string) (*Client, error) {
	conn, _, err := websocket.DefaultDialer.DialContext(ctx, url, nil)
	if err != nil {
		return nil, fmt.Errorf("substrate: dial %s: %w", url, err)
	}
	c := &Client{
		ws:      conn,
		pending: make(map[uint64]chan json.RawMessage),
		subs:    make(map[string]chan json.RawMessage),
		closed:  make(chan struct{}),
	}
	go c.readLoop()
	return c, nil
}

// Close shuts down the connection and unblocks all readers.
func (c *Client) Close() error {
	var err error
	c.closeOnce.Do(func() {
		err = c.ws.Close()
		close(c.closed)
	})
	return err
}

// Call sends a JSON-RPC request and waits for the matching reply.
func (c *Client) Call(ctx context.Context, method string, params ...any) (json.RawMessage, error) {
	id := c.nextID.Add(1)
	ch := make(chan json.RawMessage, 1)

	c.mu.Lock()
	c.pending[id] = ch
	c.mu.Unlock()

	defer func() {
		c.mu.Lock()
		delete(c.pending, id)
		c.mu.Unlock()
	}()

	req := map[string]any{
		"jsonrpc": "2.0",
		"id":      id,
		"method":  method,
		"params":  params,
	}
	c.mu.Lock()
	err := c.ws.WriteJSON(req)
	c.mu.Unlock()
	if err != nil {
		return nil, fmt.Errorf("substrate: write %s: %w", method, err)
	}

	select {
	case <-ctx.Done():
		return nil, ctx.Err()
	case <-c.closed:
		return nil, fmt.Errorf("substrate: connection closed")
	case raw := <-ch:
		return raw, nil
	}
}

// Subscribe issues an RPC method that returns a subscription ID, then
// returns a channel that yields each notification's `params.result`.
// The channel is closed when Unsubscribe is called or the connection drops.
func (c *Client) Subscribe(ctx context.Context, method, unsubMethod string, params ...any) (string, <-chan json.RawMessage, error) {
	idRaw, err := c.Call(ctx, method, params...)
	if err != nil {
		return "", nil, err
	}
	var subID string
	if err := json.Unmarshal(idRaw, &subID); err != nil {
		return "", nil, fmt.Errorf("substrate: subscribe %s: bad subscription id: %w", method, err)
	}

	ch := make(chan json.RawMessage, 16)
	c.mu.Lock()
	c.subs[subID] = ch
	c.mu.Unlock()

	go func() {
		<-ctx.Done()
		_, _ = c.Call(context.Background(), unsubMethod, subID)
		c.mu.Lock()
		if existing, ok := c.subs[subID]; ok {
			close(existing)
			delete(c.subs, subID)
		}
		c.mu.Unlock()
	}()

	return subID, ch, nil
}

func (c *Client) readLoop() {
	for {
		_, raw, err := c.ws.ReadMessage()
		if err != nil {
			slog.Debug("substrate: read loop exiting", "err", err)
			c.shutdownWaiters()
			return
		}
		var env envelope
		if err := json.Unmarshal(raw, &env); err != nil {
			slog.Warn("substrate: malformed JSON-RPC frame", "err", err)
			continue
		}
		switch {
		case env.ID != 0:
			c.deliverResponse(env)
		case env.Method != "":
			c.deliverNotification(env)
		}
	}
}

type envelope struct {
	ID     uint64          `json:"id,omitempty"`
	Method string          `json:"method,omitempty"`
	Result json.RawMessage `json:"result,omitempty"`
	Params struct {
		Subscription string          `json:"subscription"`
		Result       json.RawMessage `json:"result"`
	} `json:"params,omitempty"`
	Error *struct {
		Code    int    `json:"code"`
		Message string `json:"message"`
	} `json:"error,omitempty"`
}

func (c *Client) deliverResponse(env envelope) {
	c.mu.Lock()
	ch, ok := c.pending[env.ID]
	c.mu.Unlock()
	if !ok {
		return
	}
	if env.Error != nil {
		slog.Warn("substrate: rpc error", "code", env.Error.Code, "msg", env.Error.Message)
	}
	select {
	case ch <- env.Result:
	default:
	}
}

func (c *Client) deliverNotification(env envelope) {
	c.mu.Lock()
	ch, ok := c.subs[env.Params.Subscription]
	c.mu.Unlock()
	if !ok {
		return
	}
	select {
	case ch <- env.Params.Result:
	default:
		slog.Warn("substrate: subscription buffer full, dropping",
			"subscription", env.Params.Subscription)
	}
}

func (c *Client) shutdownWaiters() {
	c.mu.Lock()
	defer c.mu.Unlock()
	for _, ch := range c.pending {
		close(ch)
	}
	c.pending = map[uint64]chan json.RawMessage{}
	for _, ch := range c.subs {
		close(ch)
	}
	c.subs = map[string]chan json.RawMessage{}
}
