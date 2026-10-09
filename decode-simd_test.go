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

package minlz

import (
	"bytes"
	"encoding/binary"
	"errors"
	"fmt"
	"math/bits"
	"math/rand"
	"os"
	"regexp"
	"runtime/debug"
	"slices"
	"strconv"
	"strings"
	"testing"

	"github.com/klauspost/pivco"
	"github.com/minio/minlz/internal/race"
	"github.com/minio/minlz/internal/reference"
)

// The test block builder below is written from SIMD_SPEC.md, independently of the encoder.

// simdTestOp copies ll literals, then ml bytes from offset off back. off 0 repeats the offset.
type simdTestOp struct{ ll, ml, off int }

// simdTestRun executes ops as specified, and appends the literals left.
func simdTestRun(ops []simdTestOp, lits []byte) []byte {
	var out []byte
	l, last := 0, 1
	for _, o := range ops {
		out = append(out, lits[l:l+o.ll]...)
		l += o.ll
		if o.off != 0 {
			last = o.off
		}
		r := min(last, len(out))
		for range o.ml {
			// Only invalid blocks match at position 0.
			if r == 0 {
				out = append(out, 0)
				continue
			}
			out = append(out, out[len(out)-r])
		}
	}
	return append(out, lits[l:]...)
}

func simdTestRvarint(b []byte, v uint64) []byte {
	t := binary.AppendUvarint(nil, v)
	slices.Reverse(t)
	return append(b, t...)
}

func simdTestNibbles(l []uint8) []byte {
	b := make([]byte, (len(l)+1)/2)
	for i, v := range l {
		b[i/2] |= v << (4 * (i & 1))
	}
	return b
}

type simdTestStream struct {
	mode  int
	table []byte
	data  []byte
}

func (s *simdTestStream) appendEntry(b []byte) []byte {
	return append(binary.AppendUvarint(b, uint64(len(s.data))<<3|uint64(s.mode)), s.table...)
}

type simdTestChunk struct {
	flags byte
	s     [5]simdTestStream
	ops   []simdTestOp
	vals  [5][]byte
}

// simdTestEnc is a block in parts, so tests can change them.
type simdTestEnc struct {
	typ, size     int
	flags         byte
	lits          []simdTestStream
	chunks        []simdTestChunk
	noLZ          bool
	litExtra      []byte // literal data without an entry
	lzExtra       []byte
	litRecExtra   []byte
	chunkRecExtra []byte
	stored        []byte // literals as stored
}

func (e *simdTestEnc) bytes() []byte {
	out := binary.AppendUvarint([]byte{0}, uint64(e.typ)<<24|uint64(e.size))
	out = append(out, e.flags)
	start := len(out)
	var recs []byte
	if !e.noLZ {
		for i := range e.chunks {
			c := &e.chunks[i]
			recs = append(recs, c.flags)
			for k := range c.s {
				out = append(out, c.s[k].data...)
				recs = c.s[k].appendEntry(recs)
			}
		}
		out = append(out, e.lzExtra...)
		recs = append(recs, e.chunkRecExtra...)
		out = simdTestRvarint(append(out, recs...), uint64(len(recs)))
	}
	lz := len(out) - start
	recs = recs[:0]
	for i := range e.lits {
		out = append(out, e.lits[i].data...)
		recs = e.lits[i].appendEntry(recs)
	}
	out = append(out, e.litExtra...)
	recs = append(recs, e.litRecExtra...)
	out = simdTestRvarint(append(out, recs...), uint64(len(recs)))
	return simdTestRvarint(out, uint64(lz))
}

// simdTestBlock describes a block for simdTestBuilder.
type simdTestBlock struct {
	ops      []simdTestOp
	lits     []byte // output literal values
	chunk    int    // operations per chunk
	limit    int    // chunk limit value
	delta    uint64 // chunk i has delta literals if bit i%64 is set
	litBlock int    // literals per literal block
	modes    []int  // entry modes, cycled
	litModes []int
}

type simdTestBuilder struct {
	t   testing.TB
	enc *pivco.Encoder
	cur [simdLits + 1]*pivco.Table
}

func newSIMDTestBuilder(t testing.TB) *simdTestBuilder {
	enc, err := pivco.NewEncoder()
	if err != nil {
		t.Fatal(err)
	}
	return &simdTestBuilder{t: t, enc: enc}
}

// simdTestClassLens returns class lengths that give every value of stream k a code.
func simdTestClassLens(k, mode int) (cl []uint8, lens [256]uint8) {
	trunc := func(n int) []uint8 {
		k := bits.Len(uint(n)) - 1
		l := make([]uint8, n)
		for i := range l {
			l[i] = uint8(k)
			if i >= 1<<(k+1)-n {
				l[i]++
			}
		}
		return l
	}
	lay := &simdLayouts[k]
	if mode == simdDeltaLens {
		lay = &simdLayouts[simdDelta]
	}
	cl = trunc(lay.n)
	if k == simdTok {
		cl = append(cl, 2, 2, 2, 2, 2, 2, 2, 2)
		for v := range 136 {
			lens[v] = cl[lay.class[v>>2]] + lay.in[v>>2] + 2
		}
		return cl, lens
	}
	for v, c := range lay.class {
		if c != 0xff {
			lens[v] = cl[c] + lay.in[v]
		}
	}
	return cl, lens
}

// stream codes the values of stream k with mode, or stores them if that isn't possible.
func (b *simdTestBuilder) stream(k, mode int, v []byte) simdTestStream {
	s := simdTestStream{mode: mode}
	if len(v) == 0 || mode == simdStored || mode == simdDeltaLens && k != simdLits {
		s.mode, s.data = simdStored, v
		return s
	}
	t := b.cur[k]
	if mode == simdCurrent && t != nil {
		if d, err := b.enc.AppendEncodeBlock(nil, v, t); err == nil {
			s.data = d
			return s
		}
	}
	var lens [256]uint8
	switch mode {
	case simdClassLens, simdDeltaLens:
		var cl []uint8
		cl, lens = simdTestClassLens(k, mode)
		s.table = simdTestNibbles(cl)
	default:
		s.mode = simdCodeLens
		var h [256]uint64
		for _, c := range v {
			h[c]++
		}
		var tt pivco.Table
		if err := b.enc.BuildTable(&tt, &h); err != nil {
			b.t.Fatal(err)
		}
		lens = tt.Lengths()
		s.table = simdTestNibbles(lens[:2*simdCodeBytes[k]])
	}
	t = new(pivco.Table)
	if err := t.SetLengths(&lens, pivco.FlatVertical); err != nil {
		b.t.Fatal(err)
	}
	b.cur[k] = t
	d, err := b.enc.AppendEncodeBlock(nil, v, t)
	if err != nil {
		b.t.Fatal(err)
	}
	s.data = d
	return s
}

// simdTestOffset returns the offset symbol of off, its raw bits and their number.
func simdTestOffset(off int) (sym byte, raw uint64, n int) {
	if off < 16 {
		return byte(off), 0, 0
	}
	b := bits.Len(uint(off)) - 1
	return byte(8*b - 16 + off>>(b-3)&7), uint64(off) & (1<<(b-3) - 1), b - 3
}

// simdTestValues returns the values of the streams of ops.
func simdTestValues(ops []simdTestOp) (s [5][]byte) {
	var acc uint64
	var nacc int
	for _, o := range ops {
		ll, ml := o.ll, o.ml
		var t byte
		if ll > 0 {
			t |= 2
			if ll > simdMaxLen {
				s[simdEsc] = append(s[simdEsc], byte(ll-simdMaxLen-1))
				ll = simdMaxLen + 1
			}
			s[simdLL] = append(s[simdLL], byte(ll))
		}
		if ml > simdMaxLen {
			s[simdEsc] = append(s[simdEsc], byte(ml-simdMaxLen-1))
			ml = simdMaxLen + 1
		}
		t |= byte(ml << 2)
		if o.off == 0 {
			t |= 1
		} else {
			sym, raw, n := simdTestOffset(o.off)
			s[simdOffc] = append(s[simdOffc], sym)
			acc |= raw << nacc
			for nacc += n; nacc >= 8; nacc -= 8 {
				s[4] = append(s[4], byte(acc))
				acc >>= 8
			}
		}
		s[simdTok] = append(s[simdTok], t)
	}
	if nacc > 0 {
		s[4] = append(s[4], byte(acc))
	}
	return s
}

// build returns the block parts and the decoded output.
func (b *simdTestBuilder) build(tb simdTestBlock) (*simdTestEnc, []byte) {
	b.cur = [len(b.cur)]*pivco.Table{}
	out := simdTestRun(tb.ops, tb.lits)
	if tb.chunk == 0 {
		tb.chunk = 1024 << tb.limit
	}
	if tb.litBlock == 0 {
		tb.litBlock = pivco.MaxBlockSize
	}
	modes, litModes := tb.modes, tb.litModes
	if len(modes) == 0 {
		modes = []int{simdStored}
	}
	if len(litModes) == 0 {
		litModes = []int{simdStored}
	}
	e := &simdTestEnc{typ: simdBlockType, size: len(out), flags: byte(tb.limit), stored: bytes.Clone(tb.lits)}
	p, l, last := 0, 0, 1
	for i, o := range tb.ops {
		ci := i / tb.chunk
		if tb.delta>>(ci&63)&1 != 0 {
			r := min(last, p)
			for j := range min(o.ll, 32) {
				if r > 0 {
					e.stored[l+j] -= out[p-r+j%r]
				}
			}
		}
		p += o.ll + o.ml
		l += o.ll
		if o.off != 0 {
			last = o.off
		}
	}
	for i := 0; i < len(e.stored); i += tb.litBlock {
		v := e.stored[i:min(i+tb.litBlock, len(e.stored))]
		e.lits = append(e.lits, b.stream(simdLits, litModes[len(e.lits)%len(litModes)], v))
	}
	for i := 0; i < len(tb.ops); i += tb.chunk {
		c := simdTestChunk{ops: tb.ops[i:min(i+tb.chunk, len(tb.ops))]}
		ci := len(e.chunks)
		if tb.delta>>(ci&63)&1 != 0 {
			c.flags = 1
		}
		c.vals = simdTestValues(c.ops)
		for k := range 4 {
			c.s[k] = b.stream(k, modes[(ci+k)%len(modes)], c.vals[k])
		}
		c.s[4] = simdTestStream{data: c.vals[4]}
		e.chunks = append(e.chunks, c)
	}
	return e, out
}

// simdTestOps returns random operations and their literals.
// Literals are drawn from alphabet, matches are at most maxML and offsets at most maxOff.
func simdTestOps(rng *rand.Rand, n int, alphabet string, maxML, maxOff int) ([]simdTestOp, []byte) {
	var ops []simdTestOp
	var lits []byte
	pos, last := 0, 1
	for i := range n {
		var o simdTestOp
		if i == 0 || rng.Intn(3) > 0 {
			o.ll = 1 + rng.Intn(8)
			if rng.Intn(20) == 0 {
				o.ll = 1 + rng.Intn(simdMaxLen+256)
			}
		}
		for range o.ll {
			lits = append(lits, alphabet[rng.Intn(len(alphabet))])
		}
		pos += o.ll
		o.ml = rng.Intn(min(maxML, simdMaxLen) + 1)
		if rng.Intn(10) == 0 {
			o.ml = rng.Intn(maxML + 1)
		}
		switch {
		case rng.Intn(4) == 0:
			// Repeat.
		case rng.Intn(20) == 0:
			// Clamped to the position.
			o.off = 1 + rng.Intn(min(maxOff, 1<<24-1))
		default:
			o.off = 1 + rng.Intn(max(1, min(pos, maxOff)))
		}
		r := last
		if o.off != 0 {
			r = o.off
		}
		if o.ml > simdMaxLen && min(r, pos) < simdMaxLen {
			o.ml = simdMaxLen
		}
		if o.off != 0 {
			last = o.off
		}
		pos += o.ml
		ops = append(ops, o)
	}
	for range rng.Intn(50) {
		lits = append(lits, alphabet[rng.Intn(len(alphabet))])
	}
	return ops, lits
}

// simdTestPad appends repeats until the block is 64 bytes smaller than its output.
func simdTestPad(b *simdTestBuilder, tb *simdTestBlock) (*simdTestEnc, []byte) {
	for {
		e, out := b.build(*tb)
		if len(e.bytes())-1+simdMinSaving <= len(out) {
			return e, out
		}
		for range 32 {
			tb.ops = append(tb.ops, simdTestOp{ml: simdMaxLen})
		}
	}
}

func simdTestDecode(t testing.TB, block, want []byte) {
	t.Helper()
	n, err := DecodedLen(block)
	if err != nil {
		t.Fatal(err)
	}
	if n != len(want) {
		t.Fatalf("decoded length %d, want %d", n, len(want))
	}
	// Guard bytes after the capacity.
	buf := bytes.Repeat([]byte{0xfe}, n+64)
	got, err := Decode(buf[:0:n], block)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(got, want) {
		for i := range got {
			if got[i] != want[i] {
				t.Fatalf("mismatch at %d of %d: got %d, want %d", i, len(want), got[i], want[i])
			}
		}
	}
	for _, v := range buf[n:] {
		if v != 0xfe {
			t.Fatal("wrote past dst")
		}
	}
}

func TestSIMDRvarint(t *testing.T) {
	for _, tc := range []struct {
		v uint64
		b []byte
	}{{5, []byte{0x05}}, {300, []byte{0x02, 0xac}}, {100000, []byte{0x06, 0x8d, 0xa0}}} {
		if got := simdTestRvarint(nil, tc.v); !bytes.Equal(got, tc.b) {
			t.Errorf("%d: got %x, want %x", tc.v, got, tc.b)
		}
		v, n := rvarint(append([]byte{0x80, 0xff}, tc.b...))
		if v != tc.v || n != len(tc.b) {
			t.Errorf("%x: got %d, %d", tc.b, v, n)
		}
	}
	for _, v := range []uint64{0, 127, 128, 16383, 16384, 1 << 21, 1<<35 + 1, 1<<63 + 5, ^uint64(0)} {
		b := simdTestRvarint([]byte{0x80}, v)
		if got, n := rvarint(b); got != v || n != len(b)-1 {
			t.Errorf("%d: got %d, %d", v, got, n)
		}
		if _, n := rvarint(b[2:]); n != 0 && v >= 128 {
			t.Errorf("%d: truncated varint accepted", v)
		}
	}
	if _, n := rvarint(bytes.Repeat([]byte{0x80}, 10)); n != 0 {
		t.Error("unterminated varint accepted")
	}
	if _, n := rvarint(append([]byte{2}, bytes.Repeat([]byte{0xff}, 9)...)); n != 0 {
		t.Error("overflow accepted")
	}
}

func TestSIMDOffsets(t *testing.T) {
	next := 1
	for s := 1; s <= 175; s++ {
		base, n := int(simdOffTab[s]&0xffffff), int(simdOffTab[s]>>24)
		if base != next {
			t.Fatalf("symbol %d: base %d, want %d", s, base, next)
		}
		next = base + 1<<n
		for _, off := range []int{base, base + 1<<n - 1} {
			if sym, raw, nb := simdTestOffset(off); int(sym) != s || nb != n || int(raw) != off-base {
				t.Fatalf("offset %d: symbol %d/%d bits, want %d/%d", off, sym, nb, s, n)
			}
		}
	}
	if next != 1<<24 {
		t.Fatalf("offsets end at %d", next)
	}
	for s, want := range map[int][2]int{16: {16, 17}, 24: {32, 35}, 175: {15728640, 16777215}} {
		base, n := int(simdOffTab[s]&0xffffff), int(simdOffTab[s]>>24)
		if base != want[0] || base+1<<n-1 != want[1] {
			t.Errorf("symbol %d: %d-%d, want %v", s, base, base+1<<n-1, want)
		}
	}
	if simdOffTab[0] != 1 {
		t.Error("symbol 0 must decode as offset 1")
	}
}

// TestSIMDLayoutsSpec compares the class layouts with the arrays in SIMD_SPEC.md.
func TestSIMDLayoutsSpec(t *testing.T) {
	spec, err := os.ReadFile("SIMD_SPEC.md")
	if err != nil {
		t.Fatal(err)
	}
	streams := map[string]int{"A.1": simdLL, "A.2": simdEsc, "A.3": simdOffc, "A.4": simdTok, "A.5": simdLits, "A.6": simdDelta}
	caption := regexp.MustCompile(`^(Class|In-class length), [a-z ]+ (\d+)-(\d+):$`)
	found := map[string]int{}
	section, kind, lo, idx := "", "", 0, 0
	for line := range strings.Lines(strings.ReplaceAll(string(spec), "\r", "")) {
		line = strings.TrimSpace(line)
		if strings.HasPrefix(line, "## A.") {
			section, kind = line[3:6], ""
			continue
		}
		k, ok := streams[section]
		if !ok {
			continue
		}
		if m := caption.FindStringSubmatch(line); m != nil {
			kind, idx = m[1], 0
			lo, _ = strconv.Atoi(m[2])
			hi, _ := strconv.Atoi(m[3])
			found[section+kind] = hi - lo + 1
			continue
		}
		if line == "" || line == "```" {
			kind = ""
			continue
		}
		if kind == "" {
			continue
		}
		for f := range strings.SplitSeq(line, ",") {
			if f = strings.TrimSpace(f); f == "" {
				continue
			}
			want, err := strconv.Atoi(f)
			if err != nil {
				t.Fatalf("%s: %q", section, line)
			}
			v, l := lo+idx, &simdLayouts[k]
			got := int(l.class[v])
			if kind != "Class" {
				got = int(l.in[v])
			}
			if got != want {
				t.Errorf("%s %s of %d: got %d, want %d", section, kind, v, got, want)
			}
			idx++
			found[section+kind]--
		}
	}
	for s := range streams {
		for _, kind := range []string{"Class", "In-class length"} {
			if n, ok := found[s+kind]; !ok || n != 0 {
				t.Errorf("%s %s: %d values missing", s, kind, n)
			}
		}
	}
	// Every in-class code is complete, and the table sizes are those of 4.2.
	for k, want := range map[int]int{simdTok: 7, simdLL: 6, simdOffc: 12, simdEsc: 8, simdLits: 7, simdDelta: 5} {
		l := &simdLayouts[k]
		var kraft [24]float64
		for v, c := range l.class {
			if c != 0xff {
				kraft[c] += 1 / float64(int(1)<<l.in[v])
			}
		}
		for c := range l.n {
			if kraft[c] != 1 {
				t.Errorf("stream %d class %d: Kraft sum %v", k, c, kraft[c])
			}
		}
		if got := (l.n + 1) / 2; got != want {
			t.Errorf("stream %d: %d bytes, want %d", k, got, want)
		}
	}
}

func TestSIMDDecodeBuilt(t *testing.T) {
	rng := rand.New(rand.NewSource(1))
	b := newSIMDTestBuilder(t)
	modes := []int{simdStored, simdCodeLens, simdCurrent, simdClassLens, simdCurrent}
	litModes := []int{simdStored, simdCodeLens, simdCurrent, simdClassLens, simdDeltaLens}
	for i := range 40 {
		ops, lits := simdTestOps(rng, 1+rng.Intn(5000), "aaabbcdef\x00\xff", 288, 1<<rng.Intn(25))
		tb := simdTestBlock{ops: ops, lits: lits, limit: rng.Intn(6), chunk: 1 + rng.Intn(1500), litBlock: 1 + rng.Intn(9000),
			modes: modes[:1+i%len(modes)], litModes: litModes[:1+i%len(litModes)]}
		tb.chunk = min(tb.chunk, 1024<<tb.limit)
		if i%3 == 1 {
			tb.delta = rng.Uint64()
		}
		e, want := simdTestPad(b, &tb)
		t.Run(fmt.Sprint(i), func(t *testing.T) {
			simdTestDecode(t, e.bytes(), want)
		})
	}
}

// TestSIMDStoredKeepsTable checks that a stored entry keeps the current table (SIMD_SPEC.md 2.2),
// in the chunks and the literal blocks.
func TestSIMDStoredKeepsTable(t *testing.T) {
	ops := make([]simdTestOp, 3*64)
	for i := range ops {
		ops[i] = simdTestOp{ll: 4, ml: 200, off: 40}
	}
	// The first match must not be clamped below 32 (4.3).
	ops[0].ll = 40
	b := newSIMDTestBuilder(t)
	modes := []int{simdCodeLens, simdStored, simdCurrent}
	e, want := b.build(simdTestBlock{ops: ops, lits: bytes.Repeat([]byte("abcd"), len(ops)+9),
		chunk: 64, litBlock: 256, modes: modes, litModes: modes})
	for i, m := range modes {
		if e.chunks[i].s[simdTok].mode != m || e.lits[i].mode != m {
			t.Fatalf("entry %d: token mode %d, literal mode %d, want %d", i, e.chunks[i].s[simdTok].mode, e.lits[i].mode, m)
		}
	}
	simdTestDecode(t, e.bytes(), want)
	if ref, err := reference.DecodeSIMDBlock(e.bytes()); err != nil || !bytes.Equal(ref, want) {
		t.Fatal("reference:", err)
	}
}

func TestSIMDDeltaExample(t *testing.T) {
	// SIMD_SPEC.md 4.5: the output ends with 41 42 43 07 00 41 42 43, and the repeat offset is 5.
	lits := []byte{0x41, 0x42, 0x43, 0x07, 0x00, 0x41, 0x42, 0x43, 0x07, 0x01, 0x41, 0x42, 0x44, 0x07}
	ops := []simdTestOp{{ll: 8, ml: 0, off: 5}, {ll: 6, ml: 4, off: 0}}
	b := newSIMDTestBuilder(t)
	tb := simdTestBlock{ops: ops, lits: lits, chunk: 1, delta: 2}
	e, want := simdTestPad(b, &tb)
	if !bytes.Equal(e.stored[8:], []byte{0x00, 0x01, 0x00, 0x00, 0x01, 0x00}) {
		t.Fatalf("stored %x", e.stored[8:])
	}
	simdTestDecode(t, e.bytes(), want)
}

// simdTestExecs are the operation loops to compare. dispatch runs the assembly loops where there are any.
var simdTestExecs = []struct {
	name string
	fn   func(dst, lits []byte, c *simdChunk, n int, st *[4]int)
}{{"go", simdExecGo}, {"dispatch", simdExec}, {"exact", func(dst, lits []byte, c *simdChunk, n int, st *[4]int) {
	simdExecExact(dst, lits, &simdChunk{tok: c.tok[:n], ll: c.ll, offc: c.offc, esc: c.esc, raw: c.raw, delta: c.delta}, 0, [4]int{}, st)
}}}

func TestSIMDExec(t *testing.T) {
	rng := rand.New(rand.NewSource(2))
	b := newSIMDTestBuilder(t)
	type tcase struct {
		ops  []simdTestOp
		lits []byte
		bad  bool // a match longer than 32 bytes has an offset below 32
	}
	var cases []tcase
	for d0 := 1; d0 <= 40; d0 += 3 {
		for off := 1; off <= 40; off++ {
			for ml := 0; ml <= 40; ml += 3 {
				lits := make([]byte, d0+40)
				rng.Read(lits)
				// The second match starts at d0+5.
				bad := ml > simdMaxLen && min(off, d0+5) < simdMaxLen
				cases = append(cases, tcase{[]simdTestOp{{ll: d0, ml: 3, off: 7}, {ll: 2, ml: ml, off: off}, {ll: 35, ml: 5, off: 0}}, lits, bad})
			}
		}
	}
	for range 200 {
		ops, lits := simdTestOps(rng, 1+rng.Intn(300), "abcab\x00", 288, 1<<rng.Intn(16))
		cases = append(cases, tcase{ops, lits, false})
	}
	for _, ex := range simdTestExecs {
		for i, tc := range cases {
			for _, delta := range []bool{false, true} {
				tb := simdTestBlock{ops: tc.ops, lits: tc.lits, chunk: len(tc.ops)}
				if delta {
					tb.delta = 1
				}
				e, want := b.build(tb)
				v := e.chunks[0].vals
				pad := func(b []byte) []byte { return append(bytes.Clone(b), make([]byte, simdPad)...) }
				c := simdChunk{tok: v[simdTok], ll: pad(v[simdLL]), offc: pad(v[simdOffc]), esc: pad(v[simdEsc]), raw: pad(v[4]), delta: delta}
				dst := make([]byte, len(want)+simdSlack+300)
				lits := pad(e.stored)
				st := [4]int{0, 0, 1, 0}
				ex.fn(dst, lits, &c, len(c.tok), &st)
				if tc.bad {
					if st[3] == 0 {
						t.Fatalf("%s case %d delta %v: long match below offset 32 not flagged", ex.name, i, delta)
					}
					continue
				}
				d, l := st[0], st[1]
				copy(dst[d:], lits[l:len(e.stored)])
				if st[3] != 0 || d+len(e.stored)-l != len(want) || !bytes.Equal(dst[:len(want)], want) {
					t.Fatalf("%s case %d delta %v: state %v, output mismatch %v", ex.name, i, delta, st, !bytes.Equal(dst[:len(want)], want))
				}
			}
		}
	}
}

// TestSIMDTail moves the end of the block over the last operations, so the exact loop takes over at each of them.
// TestSIMDLongMatchTail checks the long match rule (4.3) on the last operation of a block,
// which runs in simdExecExact.
func TestSIMDLongMatchTail(t *testing.T) {
	b := newSIMDTestBuilder(t)
	for _, tc := range []struct {
		last simdTestOp
		ok   bool
	}{
		{simdTestOp{ll: 1, ml: 40, off: 32}, true},
		{simdTestOp{ll: 1, ml: 40, off: 20}, false},
		{simdTestOp{ll: 1, ml: 40}, false}, // The repeat offset is 20.
	} {
		ops := []simdTestOp{{ll: 40, ml: 288, off: 32}, {ml: 288}, {ml: 288}, {ll: 1, ml: 4, off: 20}, tc.last}
		// No literals follow the operations, so the last one ends the output.
		e, want := b.build(simdTestBlock{ops: ops, lits: bytes.Repeat([]byte("abcdef"), 7)})
		block := e.bytes()
		if len(block)-1+simdMinSaving > len(want) {
			t.Fatal("block too large")
		}
		if tc.ok {
			simdTestDecode(t, block, want)
			continue
		}
		if _, err := Decode(nil, block); !errors.Is(err, ErrCorrupt) {
			t.Errorf("last operation %+v: %v", tc.last, err)
		}
	}
}

func TestSIMDTail(t *testing.T) {
	rng := rand.New(rand.NewSource(3))
	b := newSIMDTestBuilder(t)
	for tail := range 40 {
		for _, delta := range []uint64{0, 1} {
			ops, lits := simdTestOps(rng, 200, "abcd", 288, 1000)
			for range 20 {
				ops = append(ops, simdTestOp{ll: rng.Intn(3), ml: rng.Intn(4), off: rng.Intn(2) * (1 + rng.Intn(10))})
			}
			n := 0
			for _, o := range ops {
				n += o.ll
			}
			for len(lits) < n+tail {
				lits = append(lits, 'x')
			}
			tb := simdTestBlock{ops: ops, lits: lits[:n+tail], delta: delta}
			e, want := simdTestPad(b, &tb)
			simdTestDecode(t, e.bytes(), want)
		}
	}
}

func TestSIMDValidation(t *testing.T) {
	b := newSIMDTestBuilder(t)
	type tcase struct {
		name  string
		modes []int
		ok    bool
		ops   []simdTestOp // replace the operations
		chunk int
		// change changes the block, and returns its output.
		change func(e *simdTestEnc, want []byte) []byte
		raw    func(b []byte) []byte
	}
	stored, coded := []int{simdStored}, []int{simdCodeLens, simdCurrent, simdClassLens}
	chunk := func(e *simdTestEnc, k int) *simdTestStream { return &e.chunks[0].s[k] }
	same := func(f func(e *simdTestEnc)) func(e *simdTestEnc, want []byte) []byte {
		return func(e *simdTestEnc, want []byte) []byte { f(e); return want }
	}
	litsOnly := func(noLZ bool) func(e *simdTestEnc, want []byte) []byte {
		return func(e *simdTestEnc, _ []byte) []byte {
			lits := bytes.Repeat([]byte("aaab"), 400)
			*e = simdTestEnc{typ: simdBlockType, size: len(lits), noLZ: noLZ, lits: []simdTestStream{newSIMDTestBuilder(t).stream(simdLits, simdCodeLens, lits)}}
			return lits
		}
	}
	trim := func(k int) func(e *simdTestEnc, want []byte) []byte {
		return same(func(e *simdTestEnc) { s := chunk(e, k); s.data = s.data[:len(s.data)-1] })
	}
	extend := func(k int) func(e *simdTestEnc, want []byte) []byte {
		return same(func(e *simdTestEnc) { s := chunk(e, k); s.data = append(s.data, 1) })
	}
	cases := []tcase{
		{name: "valid", ok: true},
		{name: "valid coded", modes: coded, ok: true},
		{name: "valid class", modes: []int{simdClassLens}, ok: true},
		{name: "type 2", change: same(func(e *simdTestEnc) { e.typ = 2 })},
		{name: "type 255", change: same(func(e *simdTestEnc) { e.typ = 255 })},
		{name: "size above 8MiB", change: same(func(e *simdTestEnc) { e.size = MaxBlockSize + 1 })},
		{name: "size+1", change: same(func(e *simdTestEnc) { e.size++ })},
		{name: "size-1", change: same(func(e *simdTestEnc) { e.size-- })},
		{name: "flags bit 5", change: same(func(e *simdTestEnc) { e.flags |= 1 << 5 })},
		{name: "flags bit 6", change: same(func(e *simdTestEnc) { e.flags |= 1 << 6 })},
		{name: "flags bit 7", change: same(func(e *simdTestEnc) { e.flags |= 1 << 7 })},
		{name: "chunk limit 5", ok: true, change: same(func(e *simdTestEnc) { e.flags |= 5 })},
		{name: "chunk limit 6", change: same(func(e *simdTestEnc) { e.flags |= 6 })},
		{name: "chunk limit 7", change: same(func(e *simdTestEnc) { e.flags |= 7 })},
		{name: "hint bits", ok: true, change: same(func(e *simdTestEnc) { e.flags |= 3 << 3 })},
		{name: "no flags", raw: func(b []byte) []byte { return b[:5] }},
		{name: "L too large", raw: func(b []byte) []byte {
			_, n := rvarint(b)
			return simdTestRvarint(b[:len(b)-n], uint64(len(b)))
		}},
		{name: "L unterminated", raw: func(b []byte) []byte { return append(b[:6], 0x80, 0x80) }},
		{name: "literal record extra", change: same(func(e *simdTestEnc) { e.litRecExtra = []byte{0x80} })},
		{name: "literal record for missing data", change: same(func(e *simdTestEnc) { e.litRecExtra = []byte{1 << 3} })},
		{name: "literal data extra", change: same(func(e *simdTestEnc) { e.litExtra = []byte{1} })},
		{name: "chunk data extra", change: same(func(e *simdTestEnc) { e.lzExtra = []byte{1} })},
		{name: "chunk record truncated", change: same(func(e *simdTestEnc) { e.chunkRecExtra = []byte{0} })},
		{name: "no literal blocks", change: same(func(e *simdTestEnc) { e.lits = nil })},
		{name: "literal block empty", change: same(func(e *simdTestEnc) { e.lits = append(e.lits, simdTestStream{}) })},
		{name: "literal block of 65536", change: same(func(e *simdTestEnc) {
			e.size += 65536
			e.lits = append(e.lits, simdTestStream{data: make([]byte, 65536)})
		})},
		{name: "literal block of 65535", ok: true, change: func(e *simdTestEnc, want []byte) []byte {
			e.size += 65535
			e.lits = append(e.lits, simdTestStream{data: make([]byte, 65535)})
			return append(want, make([]byte, 65535)...)
		}},
		{name: "literals above size", change: same(func(e *simdTestEnc) {
			e.lits = append(e.lits, simdTestStream{data: make([]byte, e.size)})
		})},
		{name: "chunk without operations", change: same(func(e *simdTestEnc) {
			e.chunks = append(e.chunks, simdTestChunk{})
		})},
		{name: "chunk above limit", ops: make([]simdTestOp, 1025)},
		{name: "chunk at limit", ok: true, ops: make([]simdTestOp, 1024)},
		{name: "chunk flag bit 4", change: same(func(e *simdTestEnc) { e.chunks[0].flags |= 1 << 4 })},
		{name: "chunk flag bit 7", change: same(func(e *simdTestEnc) { e.chunks[0].flags |= 1 << 7 })},
		{name: "chunk hint bits", ok: true, change: same(func(e *simdTestEnc) { e.chunks[0].flags |= 3 << 2 })},
		{name: "chunk no-overlap flag", ok: true, change: same(func(e *simdTestEnc) { e.chunks[0].flags |= 2 })},
		{name: "literal mode 5", change: same(func(e *simdTestEnc) { e.lits[0].mode = 5 })},
		{name: "literal mode 7", change: same(func(e *simdTestEnc) { e.lits[0].mode = 7 })},
		{name: "token mode 6", change: same(func(e *simdTestEnc) { chunk(e, simdTok).mode = 6 })},
		{name: "token mode 4", modes: coded, change: same(func(e *simdTestEnc) { chunk(e, simdTok).mode = simdDeltaLens })},
		{name: "ll mode 4", modes: coded, change: same(func(e *simdTestEnc) { chunk(e, simdLL).mode = simdDeltaLens })},
		{name: "offset bits mode 1", change: same(func(e *simdTestEnc) { chunk(e, 4).mode = simdCurrent })},
		{name: "offset bits mode 2", change: same(func(e *simdTestEnc) { chunk(e, 4).mode = simdCodeLens })},
		{name: "literal mode 1 first", modes: coded, change: same(func(e *simdTestEnc) { e.lits[0].mode, e.lits[0].table = simdCurrent, nil })},
		{name: "token mode 1 first", modes: coded, change: same(func(e *simdTestEnc) { chunk(e, simdTok).mode, chunk(e, simdTok).table = simdCurrent, nil })},
		{name: "table length 12", modes: coded, change: same(func(e *simdTestEnc) { e.lits[0].table[0x61/2] = 0xcc })},
		{name: "class length 12", modes: []int{simdClassLens}, change: same(func(e *simdTestEnc) { chunk(e, simdOffc).table[0] = 0xcc })},
		// Literal class 10 has in-class lengths of 6.
		{name: "class plus in-class above 11", modes: []int{simdClassLens}, change: same(func(e *simdTestEnc) { e.lits[0].table[5] = 0x66 })},
		{name: "class pad nibble", modes: []int{simdClassLens}, change: same(func(e *simdTestEnc) { e.lits[0].table[6] |= 0x10 })},
		{name: "incomplete table", modes: coded, change: same(func(e *simdTestEnc) {
			s := chunk(e, simdTok)
			clear(s.table)
			s.table[0] = 0x22
		})},
		{name: "single value of length 2", modes: coded, change: same(func(e *simdTestEnc) {
			s := chunk(e, simdTok)
			clear(s.table)
			s.table[0] = 0x02
		})},
		{name: "ll code for 0", modes: []int{simdCodeLens}, change: same(func(e *simdTestEnc) { chunk(e, simdLL).table[0] |= 0x0f })},
		{name: "offset code for 0", modes: []int{simdCodeLens}, change: same(func(e *simdTestEnc) { chunk(e, simdOffc).table[0] |= 0x0f })},
		{name: "pivco block truncated", modes: coded, change: trim(simdTok)},
		{name: "pivco block extra", modes: coded, change: extend(simdTok)},
		{name: "token 136", change: same(func(e *simdTestEnc) { chunk(e, simdTok).data[1] = 136 })},
		{name: "ll 0", change: same(func(e *simdTestEnc) { chunk(e, simdLL).data[1] = 0 })},
		{name: "ll 34", change: same(func(e *simdTestEnc) { chunk(e, simdLL).data[1] = 34 })},
		{name: "offset symbol 0", change: same(func(e *simdTestEnc) { chunk(e, simdOffc).data[1] = 0 })},
		{name: "offset symbol 176", change: same(func(e *simdTestEnc) { chunk(e, simdOffc).data[1] = 176 })},
		{name: "ll extra", change: extend(simdLL)},
		{name: "ll missing", change: trim(simdLL)},
		{name: "offset symbol extra", change: extend(simdOffc)},
		{name: "offset symbol missing", change: trim(simdOffc)},
		{name: "escape extra", change: extend(simdEsc)},
		{name: "escape missing", change: trim(simdEsc)},
		{name: "coded ll count", modes: coded, change: same(func(e *simdTestEnc) {
			e.chunks[0].s[simdLL] = newSIMDTestBuilder(t).stream(simdLL, simdCodeLens, append(bytes.Clone(e.chunks[0].vals[simdLL]), 1))
		})},
		{name: "offset bits extra", change: extend(4)},
		{name: "offset bits missing", change: trim(4)},
		{name: "offset bits unused", ok: true, change: same(func(e *simdTestEnc) {
			for i := range e.chunks {
				c := &e.chunks[i]
				var n int
				for _, s := range c.vals[simdOffc] {
					n += int(simdOffTab[s] >> 24)
				}
				if n%8 != 0 {
					c.s[4].data[len(c.s[4].data)-1] |= 0xff << (n % 8)
				}
			}
		})},
		{name: "long match offset 20", ops: []simdTestOp{{ll: 100, ml: 40, off: 20}}},
		{name: "long match clamped", ops: []simdTestOp{{ll: 20, ml: 40, off: 40}}},
		{name: "long match repeat 20", ops: []simdTestOp{{ll: 100, ml: 30, off: 20}, {ml: 33}}},
		{name: "long match offset 32", ok: true, ops: []simdTestOp{{ll: 100, ml: 288, off: 32}}},
		{name: "offset beyond position", ok: true, ops: []simdTestOp{{ll: 10, ml: 20, off: 1000}}},
		{name: "first op without literals", ops: []simdTestOp{{ml: 20, off: 1}, {ll: 10, ml: 20, off: 1}}},
		{name: "chunk output below its operations", chunk: 8, ops: []simdTestOp{{ll: 7}, {}, {}, {}, {}, {}, {}, {}}},
		{name: "chunk output at its operations", ok: true, chunk: 8, ops: []simdTestOp{{ll: 8}, {}, {}, {}, {}, {}, {}, {}}},
		{name: "literal over-read", change: same(func(e *simdTestEnc) { e.lits = e.lits[:len(e.lits)-1] })},
		{name: "no operations", ok: true, change: litsOnly(true)},
		{name: "no chunks", ok: true, change: litsOnly(false)},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			modes := tc.modes
			if modes == nil {
				modes = stored
			}
			ops, lits := simdTestOps(rand.New(rand.NewSource(4)), 2000, "abcdefgh", 288, 4000)
			tb := simdTestBlock{ops: ops, lits: lits, modes: modes, litModes: modes, litBlock: 3000, chunk: 600}
			if tc.ops != nil {
				tb = simdTestBlock{ops: tc.ops, lits: bytes.Repeat([]byte("abc"), 3000), chunk: tc.chunk}
				if tc.ops[0] == (simdTestOp{}) {
					// One chunk of len(tc.ops) operations, with chunk limit 0.
					for i := range tc.ops {
						tc.ops[i] = simdTestOp{ll: 1, ml: 4, off: 3}
					}
					tb.chunk = len(tc.ops)
				}
			}
			e, want := simdTestPad(b, &tb)
			if tc.change != nil {
				want = tc.change(e, want)
			}
			block := e.bytes()
			if tc.raw != nil {
				block = tc.raw(block)
			}
			if tc.ok {
				simdTestDecode(t, block, want)
				return
			}
			dst := bytes.Repeat([]byte{0xfe}, len(want)+64)
			_, err := Decode(dst[:0:len(want)], block)
			if err == nil {
				t.Fatal("no error")
			}
			if !errors.Is(err, ErrCorrupt) && !errors.Is(err, ErrUnsupported) && !errors.Is(err, ErrTooLarge) {
				t.Fatal(err)
			}
			for _, v := range dst[len(want):] {
				if v != 0xfe {
					t.Fatal("wrote past dst")
				}
			}
		})
	}
	t.Run("chunks", func(t *testing.T) {
		for _, n := range []int{simdMaxChunks, simdMaxChunks + 1} {
			ops := []simdTestOp{{ll: 1, ml: simdMaxLen, off: 1}}
			for len(ops) < n {
				ops = append(ops, simdTestOp{ml: simdMaxLen})
			}
			e, want := b.build(simdTestBlock{ops: ops, lits: []byte("a"), chunk: 1})
			block := e.bytes()
			if len(e.chunks) != n || len(block)-1+simdMinSaving > len(want) {
				t.Fatalf("%d chunks, block of %d bytes for %d", len(e.chunks), len(block), len(want))
			}
			_, rerr := reference.DecodeSIMDBlock(block)
			if n <= simdMaxChunks {
				simdTestDecode(t, block, want)
				if rerr != nil {
					t.Fatalf("%d chunks: reference: %v", n, rerr)
				}
				continue
			}
			if _, err := Decode(nil, block); !errors.Is(err, ErrCorrupt) || rerr == nil {
				t.Fatalf("%d chunks: %v, reference %v", n, err, rerr)
			}
		}
	})
	t.Run("64 bytes", func(t *testing.T) {
		for _, n := range []int{100, 1000} {
			src := append(binary.AppendUvarint(nil, 1<<24|uint64(n)), make([]byte, n-64-4)...)
			if _, _, err := simdHeader(src); err != nil {
				t.Errorf("size %d: %v", n, err)
			}
			if _, _, err := simdHeader(append(src, 0)); err == nil {
				t.Errorf("size %d: 63 bytes smaller accepted", n)
			}
		}
		if _, _, err := simdHeader(binary.AppendUvarint(nil, 1<<32|100)); err == nil {
			t.Error("varint above 32 bits accepted")
		}
	})
}

func TestSIMDDecodeAllocs(t *testing.T) {
	if race.Enabled {
		t.Skip("sync.Pool drops items with -race")
	}
	defer debug.SetGCPercent(debug.SetGCPercent(-1))
	b := newSIMDTestBuilder(t)
	ops, lits := simdTestOps(rand.New(rand.NewSource(5)), 5000, "abcdef", 288, 30000)
	tb := simdTestBlock{ops: ops, lits: lits, modes: []int{simdCodeLens, simdCurrent, simdClassLens}, litModes: []int{simdCodeLens, simdDeltaLens}, chunk: 2000, limit: 1, delta: 2}
	e, want := simdTestPad(b, &tb)
	block := e.bytes()
	dst := make([]byte, len(want))
	if _, err := Decode(dst, block); err != nil {
		t.Fatal(err)
	}
	if n := testing.AllocsPerRun(20, func() { Decode(dst, block) }); n != 0 {
		t.Errorf("%v allocations", n)
	}
}

// TestSIMDReference compares Decode with reference.DecodeSIMDBlock on built blocks, and on changed ones.
func TestSIMDReference(t *testing.T) {
	rng := rand.New(rand.NewSource(6))
	b := newSIMDTestBuilder(t)
	modes := []int{simdStored, simdCodeLens, simdCurrent, simdClassLens, simdCurrent}
	litModes := []int{simdStored, simdCodeLens, simdCurrent, simdClassLens, simdDeltaLens}
	nib := func(tab []byte, i int) byte { return tab[i/2] >> (4 * (i & 1)) & 15 }
	setNib := func(tab []byte, i int, v byte) { tab[i/2] = tab[i/2]&^(15<<(4*(i&1))) | v<<(4*(i&1)) }
	// randomCode returns the lengths of a random complete code over all or some of n values.
	// Lengths are at most 6, so most class tables stay within 11 bits per value.
	randomCode := func(n int) []uint8 {
		depth, m := []uint8{0}, n
		if rng.Intn(2) == 0 {
			m = 2 + rng.Intn(n-2)
		}
		for len(depth) < m {
			if i := rng.Intn(len(depth)); depth[i] < 6 {
				depth[i]++
				depth = append(depth, depth[i])
			}
		}
		l := make([]uint8, n)
		for i, v := range rng.Perm(n)[:len(depth)] {
			l[v] = depth[i]
		}
		return l
	}
	var sd simdDecoder
	change := func(e *simdTestEnc, litBlock int) {
		op := rng.Intn(13)
		var streams []*simdTestStream
		add := func(s *simdTestStream) {
			// Table changes need a table.
			if op != 1 && op != 2 || len(s.table) > 0 {
				streams = append(streams, s)
			}
		}
		for i := range e.lits {
			add(&e.lits[i])
		}
		for i := range e.chunks {
			for k := range e.chunks[i].s {
				add(&e.chunks[i].s[k])
			}
		}
		if len(streams) == 0 {
			return
		}
		s := streams[rng.Intn(len(streams))]
		switch op {
		case 0:
			s.mode = rng.Intn(8)
		case 1:
			n := 2 * len(s.table)
			x, y := rng.Intn(n), rng.Intn(n)
			if rng.Intn(2) == 0 {
				// Value 0 is outside the range of some streams.
				x = 0
			}
			vx, vy := nib(s.table, x), nib(s.table, y)
			setNib(s.table, x, vy)
			setNib(s.table, y, vx)
		case 2:
			// The last nibble may be a pad nibble.
			used := []int{2*len(s.table) - 1}
			for x := range 2 * len(s.table) {
				if nib(s.table, x) != 0 {
					used = append(used, x)
				}
			}
			setNib(s.table, used[rng.Intn(len(used))], byte(rng.Intn(16)))
		case 3:
			if len(s.data) > 0 {
				s.data = s.data[:len(s.data)-1]
			}
		case 4:
			s.data = append(s.data, byte(rng.Intn(256)))
		case 5:
			if len(s.data) > 0 {
				s.data[rng.Intn(len(s.data))] = byte(rng.Intn(256))
			}
		case 6:
			e.size = []int{e.size - 1, e.size + 1, MaxBlockSize + 1}[rng.Intn(3)]
		case 7:
			e.typ = 1 + rng.Intn(255)
		case 8:
			e.flags ^= 1 << rng.Intn(8)
		case 9:
			e.chunks[rng.Intn(len(e.chunks))].flags ^= 1 << rng.Intn(8)
		case 10:
			i := rng.Intn(len(e.lits))
			e.lits = append(e.lits[:i], e.lits[i+1:]...)
		case 11:
			if tok := &e.chunks[0].s[simdTok]; tok.mode == simdStored {
				tok.data[0] &^= 2
			}
		case 12:
			// A random class table, coded with the decoder's table, so the reference checks its mapping.
			k := rng.Intn(simdLits + 1)
			mode, lay := simdClassLens, k
			var st *simdTestStream
			var vals []byte
			if k == simdLits {
				i := rng.Intn(len(e.lits))
				st, vals = &e.lits[i], e.stored[i*litBlock:min(i*litBlock+litBlock, len(e.stored))]
				if rng.Intn(2) == 0 {
					mode, lay = simdDeltaLens, simdDelta
				}
			} else {
				c := &e.chunks[rng.Intn(len(e.chunks))]
				st, vals = &c.s[k], c.vals[k]
			}
			cl := randomCode(simdLayouts[lay].n)
			if k == simdTok {
				cl = append(append(cl, randomCode(4)...), randomCode(4)...)
			}
			st.mode, st.table = mode, simdTestNibbles(cl)
			if _, ok := sd.setTable(k, mode, st.table); ok && len(vals) > 0 {
				if d, err := b.enc.AppendEncodeBlock(nil, vals, &sd.tab[k]); err == nil {
					st.data = d
				}
			}
		}
	}
	// Operations at the edge of validity, not all valid.
	edge := [][]simdTestOp{
		{{ll: 100, ml: 33, off: 20}},
		{{ll: 100, ml: 33, off: 32}},
		{{ll: 20, ml: 33, off: 40}},
		{{ll: 10, ml: 20, off: 1000}},
		{{ml: 20, off: 1}, {ll: 10, ml: 20, off: 1}},
	}
	// Literals of every value, so the literal class layouts matter.
	var all []byte
	for c := range 256 {
		all = append(all, byte(c))
	}
	blocks, valid := 100, 0
	if testing.Short() {
		blocks = 20
	}
	for i := range blocks {
		alphabet := "aaabbcdef\x00\xff"
		if i%2 == 0 {
			alphabet = string(all)
		}
		ops, lits := simdTestOps(rng, 1+rng.Intn(1500), alphabet, 288, 1<<rng.Intn(25))
		switch {
		case i < len(edge):
			ops, lits = edge[i], bytes.Repeat([]byte("abc"), 100)
		case i%5 == 3:
			// Single-value streams.
			ops = make([]simdTestOp, 1+rng.Intn(500))
			for j := range ops {
				ops[j] = simdTestOp{ll: 1, ml: 4, off: 3}
			}
			lits = bytes.Repeat([]byte("a"), len(ops))
		}
		tb := simdTestBlock{ops: ops, lits: lits, limit: rng.Intn(6), chunk: 32 + rng.Intn(300), litBlock: 64 + rng.Intn(2000),
			modes: modes[:1+i%len(modes)], litModes: litModes[:1+i%len(litModes)], delta: rng.Uint64()}
		if i%4 == 1 {
			tb.chunk = 1024 << tb.limit
		}
		pad, _ := simdTestPad(b, &tb)
		orig := pad.bytes()
		for j := range 60 {
			e, want := b.build(tb)
			if j%2 == 1 {
				change(e, tb.litBlock)
			}
			block := e.bytes()
			if j > 0 && j%2 == 0 {
				_, n := binary.Uvarint(block[1:])
				p := 1 + n + rng.Intn(len(block)-1-n)
				switch rng.Intn(4) {
				case 0:
					block[p] ^= 1 << rng.Intn(8)
				case 1:
					block[p] = byte(rng.Intn(256))
				case 2:
					block = append(block[:p], block[p+1:]...)
				case 3:
					block = block[:p]
				}
			}
			got, err := Decode(nil, block)
			ref, rerr := reference.DecodeSIMDBlock(block)
			if (err == nil) != (rerr == nil) || err == nil && !bytes.Equal(got, ref) || j == 0 && i >= len(edge) && !bytes.Equal(ref, want) {
				t.Fatalf("block %d change %d: Decode: %v, reference: %v, same output: %v", i, j, err, rerr, bytes.Equal(got, ref))
			}
			if err == nil && !bytes.Equal(block, orig) {
				valid++
			}
		}
	}
	t.Logf("%d changed blocks decoded", valid)
}

func TestSIMDStats(t *testing.T) {
	rng := rand.New(rand.NewSource(7))
	for i := range 20000 {
		b := make([]byte, rng.Intn(200))
		for j := range b {
			switch i % 4 {
			case 0:
				b[j] = byte(rng.Intn(256))
			case 1:
				b[j] = byte(rng.Intn(136))
			case 2:
				b[j] = byte(1 + rng.Intn(33))
			default:
				b[j] = byte(1 + rng.Intn(175))
			}
		}
		var out, nLit, nOff, nEsc, sum, nLLEsc, nb int
		tokOK, llOK, offOK := true, true, true
		for _, v := range b {
			out += int(v >> 2)
			nLit += int(v>>1) & 1
			nOff += int(v&1) ^ 1
			if v>>2 == 33 {
				nEsc++
			}
			tokOK = tokOK && v <= 135
			sum += int(v)
			if v == 33 {
				nLLEsc++
			}
			llOK = llOK && v >= 1 && v <= 33
			nb += int(simdOffTab[v] >> 24)
			offOK = offOK && v >= 1 && v <= 175
		}
		if o, l, n, e, ok := simdTokStats(b); ok != tokOK || ok && (o != out || l != nLit || n != nOff || e != nEsc) {
			t.Fatalf("%x: tokens %v %v %v %v %v", b, o, l, n, e, ok)
		}
		if a, c, ok := simdLLStats(b); ok != llOK || ok && (a != sum || c != nLLEsc) {
			t.Fatalf("%x: literal lengths %v %v %v", b, a, c, ok)
		}
		if a, ok := simdOffStats(b); ok != offOK || ok && a != nb {
			t.Fatalf("%x: offsets %v %v, want %v", b, a, ok, nb)
		}
	}
}

// TestSIMDTokenComponents checks A.4: the group lengths and both flag codes of a token class table
// must be complete codes, even when the token code they give is.
func TestSIMDTokenComponents(t *testing.T) {
	ops := []simdTestOp{{ll: 1}}
	for range 200 {
		ops = append(ops, simdTestOp{ml: 32})
	}
	b := newSIMDTestBuilder(t)
	// Both give the same token lengths.
	for _, tc := range []struct {
		ok bool
		g  []uint8
	}{
		{true, []uint8{0: 1, 12: 1, 14: 1, 2, 3, 3, 18: 2, 2, 2, 2}},
		{false, []uint8{0: 1, 12: 2, 14: 1, 2, 3, 3, 18: 1, 1, 1, 1}},
	} {
		e, want := b.build(simdTestBlock{ops: ops, lits: []byte("abc")})
		var lens [256]uint8
		grp := &simdLayouts[simdTok]
		for v := range 136 {
			f := tc.g[14+v&3]
			if v>>2 >= 32 {
				f = tc.g[18+v&3]
			}
			if gl := tc.g[grp.class[v>>2]]; gl != 0 && f != 0 {
				lens[v] = gl + grp.in[v>>2] + f
			}
		}
		var tab pivco.Table
		if err := tab.SetLengths(&lens, pivco.FlatVertical); err != nil {
			t.Fatal(err)
		}
		d, err := b.enc.AppendEncodeBlock(nil, e.chunks[0].vals[simdTok], &tab)
		if err != nil {
			t.Fatal(err)
		}
		e.chunks[0].s[simdTok] = simdTestStream{mode: simdClassLens, table: simdTestNibbles(tc.g), data: d}
		block := e.bytes()
		got, err := Decode(nil, block)
		ref, refErr := reference.DecodeSIMDBlock(block)
		if (err == nil) != tc.ok || (refErr == nil) != tc.ok {
			t.Fatalf("complete components %v: decode error %v, reference error %v", tc.ok, err, refErr)
		}
		if tc.ok && (!bytes.Equal(got, want) || !bytes.Equal(ref, want)) {
			t.Fatal("output mismatch")
		}
	}
}
