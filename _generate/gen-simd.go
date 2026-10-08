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
	. "github.com/honeycombio/avo/build"
	. "github.com/honeycombio/avo/operand"
	"github.com/honeycombio/avo/reg"
)

// simdMaxLen is the longest literal or match length without an escape.
const simdMaxLen = 32

// genSIMD generates the operation loops of SIMD blocks (SIMD_SPEC.md).
func genSIMD() {
	tab := simdPatternTable()
	otab := NewDataAddr(Symbol{Name: "·simdOffTab"}, 0)
	genSIMDExec("simdExecAVX2", false, tab, otab)
	genSIMDExec("simdExecDeltaAVX2", true, tab, otab)
	genSIMDStats()
}

// simdPatternTable returns 33 rows of 32 bytes, indexed by min(offset, 32).
// Byte i of row r is 0x80 (keep the plain loaded byte) when i < r or r >= 32,
// otherwise i % r, which indexes the first 16 source bytes broadcast to both lanes.
// Row 0 is row 1: a delta reference at position 0 repeats dst[0], which is 0.
func simdPatternTable() Mem {
	tab := GLOBL("simdPatternTab", RODATA|NOPTR)
	for r := 0; r <= simdMaxLen; r++ {
		var row [32]byte
		p := max(r, 1)
		for i := range row {
			row[i] = 0x80
			if p < simdMaxLen && i >= p {
				row[i] = byte(i % p)
			}
		}
		for j := 0; j < 32; j += 8 {
			var v uint64
			for k := 7; k >= 0; k-- {
				v = v<<8 | uint64(row[j+k])
			}
			DATA(r*32+j, U64(v))
		}
	}
	return tab
}

// simdOffset loads the next offset symbol and its raw bits: o = base + bits.
// The symbol and bits are only consumed for new offsets. t is the token; nrep is 1 for a new offset.
func simdOffset(t reg.GPVirtual, ocp, raw reg.Register, pos reg.GPVirtual, otab Mem) (o, nrep reg.GPVirtual) {
	o = GP64()
	MOVBQZX(Mem{Base: ocp}, o)
	ot := GP64()
	LEAQ(otab, ot)
	MOVL(Mem{Base: ot, Index: o, Scale: 4}, o.As32())
	nrep = GP64()
	MOVQ(t, nrep)
	ANDQ(U8(1), nrep)
	XORQ(U8(1), nrep)
	ADDQ(nrep, ocp)
	v, c := GP64(), GP64()
	MOVQ(pos, v)
	SHRQ(U8(3), v)
	MOVQ(Mem{Base: raw, Index: v, Scale: 1}, v)
	MOVQ(pos, c)
	ANDQ(U8(7), c)
	SHRXQ(c, v, v)
	MOVL(o.As32(), c.As32())
	SHRL(U8(24), c.As32())
	BZHIQ(c, v, v)
	ANDL(U32(0xffffff), o.As32())
	ADDQ(v, o)
	NEGQ(nrep)
	ANDQ(nrep, c)
	ADDQ(c, pos)
	return o, nrep
}

// genSIMDExec generates a loop that runs the first n operations of a chunk, see simdExecGo.
// With delta, the first 32 literals of each operation are added to their delta references.
func genSIMDExec(name string, delta bool, tab, otab Mem) {
	TEXT(name, NOSPLIT, "func(dst, lits, tok, ll, offc, raw, esc *byte, n int, st *[4]int)")
	Doc(name+" runs the first n operations of a chunk. st holds the output position,",
		"the literal position, the repeat offset and an error flag.", "")
	Pragma("noescape")
	dst := Load(Param("dst"), GP64())
	litp := Load(Param("lits"), GP64())
	tok := Load(Param("tok"), GP64())
	llp := Load(Param("ll"), GP64())
	ocp := Load(Param("offc"), GP64())
	raw := Load(Param("raw"), GP64())
	end := Load(Param("n"), GP64())
	ADDQ(tok, end)
	stp := Load(Param("st"), GP64())
	d, last := GP64(), GP64()
	MOVQ(Mem{Base: stp}, d)
	ADDQ(Mem{Base: stp, Disp: 8}, litp)
	MOVQ(Mem{Base: stp, Disp: 16}, last)
	pos := GP64()
	XORQ(pos, pos)
	CMPQ(tok, end)
	JAE(LabelRef(name + "_done"))

	PCALIGN(Imm(32))
	Label(name + "_loop")
	t := GP64()
	MOVBQZX(Mem{Base: tok}, t)
	lv := YMM()
	VMOVDQU(Mem{Base: litp}, lv)
	var dr, rs reg.GPVirtual
	var rv reg.VecVirtual
	if delta {
		// The references use the repeat offset from before this operation, clamped to the position.
		dr, rs, rv = GP64(), GP64(), YMM()
		MOVQ(last, dr)
		CMPQ(dr, d)
		CMOVQHI(d, dr)
		MOVQ(d, rs)
		SUBQ(dr, rs)
		CMPQ(dr, U8(simdMaxLen))
		JB(LabelRef(name + "_dpattern"))
		VMOVDQU(Mem{Base: dst, Index: rs, Scale: 1}, rv)
		Label(name + "_dadd")
		VPADDB(rv, lv, lv)
	}
	VMOVDQU(lv, Mem{Base: dst, Index: d, Scale: 1})
	llv, f := GP64(), GP64()
	MOVBQZX(Mem{Base: llp}, llv)
	MOVQ(t, f)
	SHRQ(U8(1), f)
	ANDQ(U8(1), f)
	ADDQ(f, llp)
	NEGQ(f)
	ANDQ(f, llv)
	CMPQ(llv, U8(simdMaxLen+1))
	JE(LabelRef(name + "_litlong"))
	Label(name + "_litback")
	ADDQ(llv, d)
	ADDQ(llv, litp)

	o, nrep := simdOffset(t, ocp, raw, pos, otab)
	TESTQ(nrep, nrep)
	CMOVQNE(o, last)
	// Matches copy from the repeat offset clamped to the position.
	r := GP64()
	MOVQ(last, r)
	CMPQ(r, d)
	CMOVQHI(d, r)
	src := GP64()
	MOVQ(d, src)
	SUBQ(r, src)
	SHRQ(U8(2), t)
	srcMem := Mem{Base: dst, Index: src, Scale: 1}
	dstMem := Mem{Base: dst, Index: d, Scale: 1}
	CMPQ(r, t)
	JB(LabelRef(name + "_pattern"))
	mv := YMM()
	VMOVDQU(srcMem, mv)
	VMOVDQU(mv, dstMem)
	Label(name + "_check")
	CMPQ(t, U8(simdMaxLen+1))
	JE(LabelRef(name + "_long"))
	Label(name + "_next")
	ADDQ(t, d)
	INCQ(tok)
	CMPQ(tok, end)
	JB(LabelRef(name + "_loop"))

	Label(name + "_done")
	VZEROUPPER()
	stp = Load(Param("st"), GP64())
	MOVQ(d, Mem{Base: stp})
	lits := Load(Param("lits"), GP64())
	SUBQ(lits, litp)
	MOVQ(litp, Mem{Base: stp, Disp: 8})
	MOVQ(last, Mem{Base: stp, Disp: 16})
	RET()

	pattern := func(row reg.GPVirtual, m Mem, out reg.VecVirtual) {
		tp := GP64()
		LEAQ(tab, tp)
		SHLQ(U8(5), row)
		bv, tv := YMM(), YMM()
		VMOVDQU(m, out)
		VBROADCASTI128(m, bv)
		VMOVDQU(Mem{Base: tp, Index: row, Scale: 1}, tv)
		VPSHUFB(tv, bv, bv)
		VPBLENDVB(tv, out, bv, out)
	}
	// Taken when the clamped offset is below min(ml, 33). Row 32 is a plain copy.
	Label(name + "_pattern")
	pv := YMM()
	pattern(r, srcMem, pv)
	VMOVDQU(pv, dstMem)
	JMP(LabelRef(name + "_check"))

	if delta {
		Label(name + "_dpattern")
		pattern(dr, Mem{Base: dst, Index: rs, Scale: 1}, rv)
		JMP(LabelRef(name + "_dadd"))
	}

	// Escaped literal length: 33 plus an escape byte. The first 32 literals are stored.
	// The escape cursor lives in its argument slot: only escapes touch it, and the registers are taken.
	Label(name + "_litlong")
	{
		ep := Load(Param("esc"), GP64())
		cnt := GP64()
		MOVBQZX(Mem{Base: ep}, cnt)
		INCQ(ep)
		Store(ep, Param("esc"))
		LEAQ(Mem{Base: cnt, Disp: simdMaxLen + 1}, llv)
		// 32-byte steps after the first: (ll-32+31)>>5 = (escape+32)>>5.
		ADDQ(U8(simdMaxLen), cnt)
		SHRQ(U8(5), cnt)
		s, o := GP64(), GP64()
		LEAQ(Mem{Base: litp, Disp: 32}, s)
		LEAQ(Mem{Base: d, Disp: 32}, o)
		Label(name + "_litloop")
		y := YMM()
		VMOVDQU(Mem{Base: s}, y)
		VMOVDQU(y, Mem{Base: dst, Index: o, Scale: 1})
		ADDQ(U8(32), s)
		ADDQ(U8(32), o)
		DECQ(cnt)
		JNZ(LabelRef(name + "_litloop"))
		JMP(LabelRef(name + "_litback"))
	}

	Label(name + "_long")
	{
		// Matches longer than 32 bytes need an offset of at least 32.
		dist := GP64()
		MOVQ(d, dist)
		SUBQ(src, dist)
		CMPQ(dist, U8(simdMaxLen))
		JAE(LabelRef(name + "_longok"))
		sp := Load(Param("st"), GP64())
		MOVQ(U32(1), Mem{Base: sp, Disp: 24})
		Label(name + "_longok")
		ep := Load(Param("esc"), GP64())
		cnt := GP64()
		MOVBQZX(Mem{Base: ep}, cnt)
		INCQ(ep)
		Store(ep, Param("esc"))
		LEAQ(Mem{Base: cnt, Disp: simdMaxLen + 1}, t)
		ADDQ(U8(simdMaxLen), cnt)
		SHRQ(U8(5), cnt)
		s, o := GP64(), GP64()
		LEAQ(Mem{Base: src, Disp: 32}, s)
		LEAQ(Mem{Base: d, Disp: 32}, o)
		Label(name + "_longloop")
		y := YMM()
		VMOVDQU(Mem{Base: dst, Index: s, Scale: 1}, y)
		VMOVDQU(y, Mem{Base: dst, Index: o, Scale: 1})
		ADDQ(U8(32), s)
		ADDQ(U8(32), o)
		DECQ(cnt)
		JNZ(LabelRef(name + "_longloop"))
		JMP(LabelRef(name + "_next"))
	}
}

// genSIMDStats generates the stream checks of chunks, for multiples of 32 bytes.
// See simdTokStats, simdLLStats and simdOffStats.
func genSIMDStats() {
	TEXT("simdTokStatsAVX2", NOSPLIT, "func(b *byte, n int) (out, lit, rep, esc int, ok bool)")
	Doc("simdTokStatsAVX2 returns the token statistics of n bytes, a multiple of 32.",
		"lit and esc are counted 2 and 4 times.", "")
	Pragma("noescape")
	p, end, zero, valid := simdStatsStart()
	acc := simdZeros(4)
	out, lit, rep, esc := acc[0], acc[1], acc[2], acc[3]
	m3f, m4, m2, m1, max := simdBroadcast(0x3f), simdBroadcast(4), simdBroadcast(2), simdBroadcast(1), simdBroadcast(135)
	Label("simdTokStatsAVX2_loop")
	t, x := YMM(), YMM()
	VMOVDQU(Mem{Base: p}, t)
	VPMINUB(max, t, x)
	VPCMPEQB(t, x, x)
	VPAND(x, valid, valid)
	VPSRLW(U8(2), t, x)
	VPAND(m3f, x, x)
	simdSum(x, zero, out)
	VPAND(m2, t, x)
	simdSum(x, zero, lit)
	VPAND(m1, t, x)
	simdSum(x, zero, rep)
	// Match length code 33 has bits 7 and 2.
	VPSRLW(U8(5), t, x)
	VPAND(t, x, x)
	VPAND(m4, x, x)
	simdSum(x, zero, esc)
	simdStatsNext(p, end, "simdTokStatsAVX2_loop")
	simdStatsEnd(valid, []string{"out", "lit", "rep", "esc"}, out, lit, rep, esc)

	TEXT("simdLLStatsAVX2", NOSPLIT, "func(b *byte, n int) (sum, esc int, ok bool)")
	Doc("simdLLStatsAVX2 returns the sum of n bytes, a multiple of 32, and how many are 33.",
		"ok is false if a byte is outside 1-33.", "")
	Pragma("noescape")
	p, end, zero, valid = simdStatsStart()
	acc = simdZeros(2)
	sum, cnt := acc[0], acc[1]
	c1, c33 := simdBroadcast(1), simdBroadcast(33)
	Label("simdLLStatsAVX2_loop")
	t, x = YMM(), YMM()
	VMOVDQU(Mem{Base: p}, t)
	VPMAXUB(c1, t, x)
	VPMINUB(c33, x, x)
	VPCMPEQB(t, x, x)
	VPAND(x, valid, valid)
	VPCMPEQB(c33, t, x)
	VPAND(c1, x, x)
	simdSum(x, zero, cnt)
	simdSum(t, zero, sum)
	simdStatsNext(p, end, "simdLLStatsAVX2_loop")
	simdStatsEnd(valid, []string{"sum", "esc"}, sum, cnt)

	TEXT("simdOffStatsAVX2", NOSPLIT, "func(b *byte, n int) (sum, zeros int, ok bool)")
	Doc("simdOffStatsAVX2 returns the sum of b>>3 over n bytes, a multiple of 32, and how often it is 0.",
		"ok is false if a byte is outside 1-175.", "")
	Pragma("noescape")
	p, end, zero, valid = simdStatsStart()
	acc = simdZeros(2)
	sum, cnt = acc[0], acc[1]
	c1, c175, m1f := simdBroadcast(1), simdBroadcast(175), simdBroadcast(0x1f)
	Label("simdOffStatsAVX2_loop")
	t, x = YMM(), YMM()
	VMOVDQU(Mem{Base: p}, t)
	VPMAXUB(c1, t, x)
	VPMINUB(c175, x, x)
	VPCMPEQB(t, x, x)
	VPAND(x, valid, valid)
	VPSRLW(U8(3), t, t)
	VPAND(m1f, t, t)
	VPCMPEQB(zero, t, x)
	VPAND(c1, x, x)
	simdSum(x, zero, cnt)
	simdSum(t, zero, sum)
	simdStatsNext(p, end, "simdOffStatsAVX2_loop")
	simdStatsEnd(valid, []string{"sum", "zeros"}, sum, cnt)
}

func simdStatsStart() (p, end reg.GPVirtual, zero, valid reg.VecVirtual) {
	p, end = GP64(), GP64()
	Load(Param("b"), p)
	Load(Param("n"), end)
	ADDQ(p, end)
	zero, valid = YMM(), YMM()
	VPXOR(zero, zero, zero)
	VPCMPEQB(valid, valid, valid)
	return p, end, zero, valid
}

func simdZeros(n int) []reg.VecVirtual {
	v := make([]reg.VecVirtual, n)
	for i := range v {
		v[i] = YMM()
		VPXOR(v[i], v[i], v[i])
	}
	return v
}

func simdBroadcast(c byte) reg.VecVirtual {
	r, x, y := GP32(), XMM(), YMM()
	MOVL(U32(c), r)
	VMOVD(r, x)
	VPBROADCASTB(x, y)
	return y
}

// simdSum adds the sums of each 8 bytes of x to the quadwords of acc. x is overwritten.
func simdSum(x, zero, acc reg.VecVirtual) {
	VPSADBW(zero, x, x)
	VPADDQ(x, acc, acc)
}

func simdStatsNext(p, end reg.GPVirtual, loop string) {
	ADDQ(U8(32), p)
	CMPQ(p, end)
	JB(LabelRef(loop))
}

// simdStatsEnd stores the sums of the quadwords of the accumulators in their results,
// and whether all bytes were valid in ok.
func simdStatsEnd(valid reg.VecVirtual, names []string, accs ...reg.VecVirtual) {
	m := GP32()
	VPMOVMSKB(valid, m)
	CMPL(m, U32(0xffffffff))
	ok := GP8()
	SETEQ(ok)
	Store(ok, Return("ok"))
	for i, v := range accs {
		x, y := XMM(), XMM()
		VEXTRACTI128(U8(1), v, x)
		VPADDQ(x, v.AsX(), x)
		VPSHUFD(U8(0x4e), x, y)
		VPADDQ(y, x, x)
		r := GP64()
		MOVQ(x, r)
		Store(r, Return(names[i]))
	}
	VZEROUPPER()
	RET()
}
