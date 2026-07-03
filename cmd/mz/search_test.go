// Copyright 2026 MinIO Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

package main

import (
	"bytes"
	"io"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"

	"github.com/minio/minlz"
)

// TestSearchCountLineStraddle guards against double-counting a matching line
// that straddles a block boundary and matches in both halves. `mz search -c`
// counts matching lines (grep/rg semantics), so such a line must count once.
func TestSearchCountLineStraddle(t *testing.T) {
	const block = 4096
	filler := []byte("z,Queens,zz\n") // 12 bytes, no "Manhattan"
	row := []byte("R,Manhattan,PPPPPPPPPPPPPPPPPPPP,Manhattan,E\n")
	m1 := bytes.Index(row, []byte("Manhattan"))
	m2 := bytes.Index(row[m1+1:], []byte("Manhattan")) + m1 + 1

	// Pick filler count so the row's first "Manhattan" sits in block 0 and the
	// second in block 1.
	k := 0
	for {
		rs := k * len(filler)
		if rs+m1+len("Manhattan") <= block && rs+m2 >= block {
			break
		}
		if k++; k > block {
			t.Fatal("could not position a straddling row")
		}
	}

	var data []byte
	data = append(data, bytes.Repeat(filler, k)...)
	data = append(data, row...) // straddling line, 2 matches
	data = append(data, bytes.Repeat(filler, 10)...)
	data = append(data, []byte("SECOND,Manhattan,x\n")...) // one more distinct match
	data = append(data, bytes.Repeat(filler, 10)...)

	var buf bytes.Buffer
	cfg := minlz.NewSearchTableConfig().WithMatchLen(6)
	w := minlz.NewWriter(&buf, minlz.WriterSearchTable(cfg), minlz.WriterBlockSize(block), minlz.WriterConcurrency(1))
	if _, err := w.Write(data); err != nil {
		t.Fatal(err)
	}
	if err := w.Close(); err != nil {
		t.Fatal(err)
	}
	if want := bytes.Count(data, []byte("Manhattan")); want != 3 {
		t.Fatalf("test setup: expected 3 raw matches (2 in the straddling row + 1), got %d", want)
	}

	path := filepath.Join(t.TempDir(), "straddle.mz")
	if err := os.WriteFile(path, buf.Bytes(), 0o644); err != nil {
		t.Fatal(err)
	}

	got := captureCount(t, path, "Manhattan")
	if got != 2 {
		t.Fatalf("mz search -c counted %d matching lines, want 2 (the straddling line must count once)", got)
	}
}

// captureCount runs searchFile in count/line mode and returns the printed count.
func captureCount(t *testing.T, path, pattern string) int {
	t.Helper()
	old := os.Stdout
	rp, wp, err := os.Pipe()
	if err != nil {
		t.Fatal(err)
	}
	os.Stdout = wp
	_, _, serr := searchFile(path, []byte(pattern), searchOpts{count: true, lines: true})
	wp.Close()
	os.Stdout = old
	out, _ := io.ReadAll(rp)
	if serr != nil {
		t.Fatalf("searchFile: %v", serr)
	}
	n, err := strconv.Atoi(strings.TrimSpace(string(out)))
	if err != nil {
		t.Fatalf("parsing count %q: %v", out, err)
	}
	return n
}

// TestSearchCountLongLineManyBlocks pins how a single newline-free line (e.g.
// minified JSON) is counted when its matches are spread across blocks. A
// skipped block between two matches is the previous block of the later match,
// so it is scanned for a separating newline: "contiguous" and "gapped" (one
// skipped block between matches) resolve to a single line. A wider gap of 2+
// skipped blocks is never decoded; since blocks are large a line spanning them
// is unlikely, so "widegap" assumes a newline was skipped and counts each
// matching block as its own line (4 matches -> 4).
func TestSearchCountLongLineManyBlocks(t *testing.T) {
	const block = 4096
	needle := []byte("NEEDLE")
	// hasNeedle selects which blocks carry the needle; want is the expected line
	// count under the single-block-gap continuation rule described above.
	cases := []struct {
		name      string
		hasNeedle func(i int) bool
		want      int
	}{
		{"contiguous", func(i int) bool { return true }, 1},
		{"gapped", func(i int) bool { return i%2 == 0 }, 1},
		{"widegap", func(i int) bool { return i%3 == 0 }, 4},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			var data []byte
			for i := range 10 {
				chunk := bytes.Repeat([]byte("x"), block) // no newlines: one logical line
				if tc.hasNeedle(i) {
					copy(chunk[100:], needle)
				}
				data = append(data, chunk...)
			}
			var buf bytes.Buffer
			cfg := minlz.NewSearchTableConfig().WithMatchLen(6)
			w := minlz.NewWriter(&buf, minlz.WriterSearchTable(cfg), minlz.WriterBlockSize(block), minlz.WriterConcurrency(1))
			if _, err := w.Write(data); err != nil {
				t.Fatal(err)
			}
			if err := w.Close(); err != nil {
				t.Fatal(err)
			}
			path := filepath.Join(t.TempDir(), "long.mz")
			if err := os.WriteFile(path, buf.Bytes(), 0o644); err != nil {
				t.Fatal(err)
			}
			if got := captureCount(t, path, string(needle)); got != tc.want {
				t.Fatalf("count=%d, want %d", got, tc.want)
			}
		})
	}
}
