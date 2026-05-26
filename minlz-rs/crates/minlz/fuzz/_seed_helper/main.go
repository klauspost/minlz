// Walks a Go fuzz cache directory (e.g. $GOCACHE/fuzz/github.com/minio/minlz/FuzzDecodeBlock)
// and writes each entry as a raw byte file under dst/, decoding Go's
// `go test fuzz v1` text format via strconv.Unquote.
//
// Usage: go run . <go-fuzz-cache-target-dir> <output-dir>
package main

import (
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"strings"
)

func main() {
	if len(os.Args) != 3 {
		fmt.Fprintln(os.Stderr, "usage: go run . <go-fuzz-cache-target-dir> <output-dir>")
		os.Exit(2)
	}
	src, dst := os.Args[1], os.Args[2]
	if err := os.MkdirAll(dst, 0o755); err != nil {
		fatal(err)
	}
	entries, err := os.ReadDir(src)
	if err != nil {
		fatal(err)
	}

	imported, skipped := 0, 0
	for _, e := range entries {
		if e.IsDir() {
			continue
		}
		data, err := os.ReadFile(filepath.Join(src, e.Name()))
		if err != nil {
			skipped++
			continue
		}
		bytes, err := decodeGoFuzzEntry(data)
		if err != nil {
			fmt.Fprintf(os.Stderr, "skip %s: %v\n", e.Name(), err)
			skipped++
			continue
		}
		if err := os.WriteFile(filepath.Join(dst, e.Name()), bytes, 0o644); err != nil {
			fatal(err)
		}
		imported++
	}
	fmt.Printf("%s -> %s: %d imported, %d skipped\n", src, dst, imported, skipped)
}

// decodeGoFuzzEntry parses a single `go test fuzz v1` cache file with a
// single `[]byte("…")` parameter and returns the raw byte payload.
func decodeGoFuzzEntry(data []byte) ([]byte, error) {
	lines := strings.Split(string(data), "\n")
	if len(lines) < 2 || !strings.HasPrefix(lines[0], "go test fuzz v") {
		return nil, fmt.Errorf("missing fuzz header")
	}
	for _, line := range lines[1:] {
		line = strings.TrimSpace(line)
		const prefix = "[]byte("
		if !strings.HasPrefix(line, prefix) || !strings.HasSuffix(line, ")") {
			continue
		}
		quoted := line[len(prefix) : len(line)-1]
		s, err := strconv.Unquote(quoted)
		if err != nil {
			return nil, fmt.Errorf("unquote: %w", err)
		}
		return []byte(s), nil
	}
	return nil, fmt.Errorf("no []byte parameter")
}

func fatal(err error) {
	fmt.Fprintln(os.Stderr, "error:", err)
	os.Exit(1)
}
