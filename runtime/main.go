package main

import (
	"context"
	"crypto/sha256"
	"crypto/subtle"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"log/slog"
	"net"
	"net/http"
	"os"
	"os/signal"
	"runtime/debug"
	"strings"
	"syscall"

	"github.com/aexhq/brain"
)

func authority(token string) func(*http.Request) (brain.Grant, bool) {
	expected := sha256.Sum256([]byte("Bearer " + token))
	return func(r *http.Request) (brain.Grant, bool) {
		supplied := sha256.Sum256([]byte(r.Header.Get("Authorization")))
		if subtle.ConstantTimeCompare(expected[:], supplied[:]) != 1 {
			return brain.Grant{}, false
		}
		tenant := r.Header.Get("X-Aex-Tenant")
		if tenant == "" || len(tenant) > 128 || strings.TrimSpace(tenant) != tenant {
			return brain.Grant{}, false
		}
		return brain.Grant{Tenant: tenant, Session: r.Header.Get("X-Aex-Session"), Manage: true}, true
	}
}

func run(ctx context.Context, filename string) error {
	file, err := os.Open(filename)
	if err != nil {
		return err
	}
	defer file.Close()
	cfg := applicationConfig{Config: brain.DefaultConfig()}
	decoder := json.NewDecoder(io.LimitReader(file, 4<<20))
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(&cfg); err != nil {
		return err
	}
	if err := decoder.Decode(new(any)); err != io.EOF {
		return errors.New("trailing runtime configuration")
	}
	host, _, err := net.SplitHostPort(cfg.Listen)
	if err != nil || net.ParseIP(host) == nil || !net.ParseIP(host).IsLoopback() {
		return errors.New("runtime listener must use a literal loopback address")
	}
	if len(cfg.Tokens) != 0 {
		return errors.New("runtime authority belongs to the Aex gateway")
	}
	token := os.Getenv("AEX_BRAIN_TOKEN")
	if len(token) < 32 {
		return errors.New("AEX_BRAIN_TOKEN requires at least 32 characters")
	}
	var server *brain.Server
	providers, err := managedProviders(cfg, func() sessionReader { return server })
	if err != nil {
		return err
	}
	server, err = brain.New(ctx, brain.Options{Config: cfg.Config, Providers: providers, Authenticate: authority(token), Logger: slog.Default()})
	if err != nil {
		return err
	}
	return server.Run(ctx)
}

func main() {
	filename := flag.String("config", "", "Brain application configuration")
	version := flag.Bool("version", false, "print the pinned Brain module")
	flag.Parse()
	if *version {
		info, ok := debug.ReadBuildInfo()
		if ok {
			for _, dependency := range info.Deps {
				if dependency.Path == "github.com/aexhq/brain" && dependency.Replace == nil {
					fmt.Println(dependency.Version)
					return
				}
			}
		}
		fmt.Fprintln(os.Stderr, "immutable Brain module information is unavailable")
		os.Exit(1)
	}
	slog.SetDefault(slog.New(slog.NewJSONHandler(os.Stderr, nil)))
	ctx, cancel := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer cancel()
	if err := run(ctx, *filename); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
