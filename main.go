package main

import (
	"context"
	"database/sql"
	"embed"
	"encoding/json"
	"fmt"
	"io/fs"
	"log"
	"math/big"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	_ "modernc.org/sqlite"
)

type itsNano struct{ *big.Int }

func (n itsNano) MarshalJSON() ([]byte, error) {
	if n.Int == nil {
		return json.Marshal("0")
	}
	return json.Marshal(n.String())
}

type Entry struct {
	Blud string  `json:"blud"`
	Lol  string  `json:"lol"`
	Time itsNano `json:"time"`
}

//go:embed static
var staticFS embed.FS

func itsLink() string {
	if v := os.Getenv("OFFSET_LINK"); v != "" {
		return strings.TrimRight(v, "/")
	}
	return "http://127.0.0.1:8080"
}

func calculateNsecs() (*big.Int, error) {
	resp, err := http.Get(itsLink() + "/api/nsecs")
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	var payload struct {
		Nsecs string `json:"nsecs"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&payload); err != nil {
		return nil, err
	}

	nsecs, ok := new(big.Int).SetString(payload.Nsecs, 10)
	if !ok {
		return nil, fmt.Errorf("bad nsecs: %q", payload.Nsecs)
	}
	return nsecs, nil
}

func main() {
	db, err := sql.Open("sqlite", "AC.db?_pragma=journal_mode(WAL)&_pragma=foreign_keys(1)")
	if err != nil {
		log.Fatalf("Fucked up to open db: %v", err)
	}
	defer db.Close()

	ctx := context.Background()
	createTableSQL := `CREATE TABLE IF NOT EXISTS chat (
		blud TEXT NOT NULL,
		lol TEXT NOT NULL,
		created_at BIGINT NOT NULL
	);`

	_, err = db.ExecContext(ctx, createTableSQL)
	if err != nil {
		log.Fatalf("Fucked up to create table: %v", err)
	}
	log.Println("DB initialized")

	http.HandleFunc("/api/diddy_chat", handleMessages(db))

	sub, err := fs.Sub(staticFS, "static")
	if err != nil {
		log.Fatalf("Fucked up to target static dir: %v", err)
	}
	http.Handle("/", imageCache(http.FileServer(http.FS(sub))))

	log.Fatal(http.ListenAndServe("127.0.0.1:911", nil))
}

func handleMessages(db* sql.DB) http.HandlerFunc {
	rl := newRateLimiter(5, time.Minute)

	return func(w http.ResponseWriter, r *http.Request) {
		ctx := r.Context()

		switch r.Method {
		case http.MethodGet:
			rows, err := db.QueryContext(ctx, "SELECT blud, lol, created_at FROM chat")
			if err != nil {
				http.Error(w, err.Error(), http.StatusInternalServerError)
				return
			}
			defer rows.Close()

			entries := []Entry{}
			for rows.Next() {
				var e Entry
				var t int64
				if err := rows.Scan(&e.Blud, &e.Lol, &t); err != nil {
					http.Error(w, err.Error(), http.StatusInternalServerError)
					return
				}
				e.Time = itsNano{big.NewInt(t)}
				entries = append(entries, e)
			}

			w.Header().Set("Content-Type", "application/json")
			json.NewEncoder(w).Encode(entries)

		case http.MethodPost:
			if !rl.allow(clientIP(r)) {
				http.Error(w, "chill out", http.StatusTooManyRequests)
				return
			}

			r.Body = http.MaxBytesReader(w, r.Body, 4096)
			var e Entry
			if err := json.NewDecoder(r.Body).Decode(&e); err != nil {
				http.Error(w, "Invalid JSON", http.StatusBadRequest)
				return
			}

			if e.Lol == "" {
				http.Error(w, "uh nuh", http.StatusBadRequest)
				return
			}

			if len(e.Blud) > 1000 || len(e.Lol) > 1000 {
				http.Error(w, "too long", http.StatusBadRequest)
				return
			}

			nsecs, err := calculateNsecs()
			if err != nil {
				http.Error(w, "its-site unavailable", http.StatusBadGateway)
				return
			}

			_, err = db.ExecContext(ctx, "INSERT INTO chat (blud, lol, created_at) VALUES (?, ?, ?)", e.Blud, e.Lol, nsecs.Int64())
			if err != nil {
				http.Error(w, err.Error(), http.StatusInternalServerError)
				return
			}

			w.WriteHeader(http.StatusCreated)
			w.Write([]byte(`{"status":"success"}`))

		default:
			http.Error(w, "Method not allowed", http.StatusMethodNotAllowed)
		}
	}
}

type rateLimiter struct {
	mu       sync.Mutex
	limit    int
	window   time.Duration
	requests map[string][]time.Time
}

func newRateLimiter(limit int, window time.Duration) *rateLimiter {
	return &rateLimiter{
		limit:    limit,
		window:   window,
		requests: make(map[string][]time.Time),
	}
}

func (rl *rateLimiter) allow(ip string) bool {
	rl.mu.Lock()
	defer rl.mu.Unlock()

	now := time.Now()
	cutoff := now.Add(-rl.window)
	ts := rl.requests[ip]

	i := 0
	for i < len(ts) && ts[i].Before(cutoff) {
		i++
	}
	ts = ts[i:]

	if len(ts) >= rl.limit {
		rl.requests[ip] = ts
		return false
	}

	rl.requests[ip] = append(ts, now)
	return true
}

func imageCache(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if isImage(r.URL.Path) {
			w.Header().Set("Cache-Control", "public, max-age=604800, immutable")
		}
		next.ServeHTTP(w, r)
	})
}

func isImage(path string) bool {
	switch strings.ToLower(filepath.Ext(path)) {
	case ".png", ".jpg", ".jpeg", ".webp", ".gif", ".svg", ".ico", ".avif":
		return true
	}
	return false
}

func clientIP(r *http.Request) string {
	if xff := r.Header.Get("X-Forwarded-For"); xff != "" {
		if i := strings.IndexByte(xff, ','); i != -1 {
			return strings.TrimSpace(xff[:i])
		}
		return strings.TrimSpace(xff)
	}
	if host, _, err := net.SplitHostPort(r.RemoteAddr); err == nil {
		return host
	}
	return r.RemoteAddr
}

