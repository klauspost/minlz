// Copyright 2025 MinIO Inc.
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
	"fmt"
	"math/bits"
	"sync"
)

// hash4 returns the hash of the lowest 4 bytes of u to fit in a hash table with h bits.
// Preferably h should be a constant and should always be <32.
func hash4(u uint64, h uint8) uint32 {
	const prime4bytes = 2654435761
	return (uint32(u) * prime4bytes) >> ((32 - h) & 31)
}

// hash5 returns the hash of the lowest 5 bytes of u to fit in a hash table with h bits.
// Preferably h should be a constant and should always be <64.
func hash5(u uint64, h uint8) uint32 {
	const prime5bytes = 889523592379
	return uint32(((u << (64 - 40)) * prime5bytes) >> ((64 - h) & 63))
}

// hash7 returns the hash of the lowest 7 bytes of u to fit in a hash table with h bits.
// Preferably h should be a constant and should always be <64.
func hash7(u uint64, h uint8) uint32 {
	const prime7bytes = 58295818150454627
	return uint32(((u << (64 - 56)) * prime7bytes) >> ((64 - h) & 63))
}

// hash8 returns the hash of u to fit in a hash table with h bits.
// Preferably h should be a constant and should always be <64.
func hash8(u uint64, h uint8) uint32 {
	const prime8bytes = 0xcf1bbcdcb7a56463
	return uint32((u * prime8bytes) >> ((64 - h) & 63))
}

var encLPool sync.Pool

// encodeBlockBetter encodes a non-empty src to a guaranteed-large-enough dst. It
// assumes that the varint-encoded length of the decompressed bytes has already
// been written.
//
// It also assumes that:
//
//	len(dst) >= MaxEncodedLen(len(src)) &&
//	minNonLiteralBlockSize <= len(src) && len(src) <= maxBlockSize
func encodeBlockBetterGo(dst, src []byte) (d int) {
	// sLimit is when to stop looking for offset/length copies. The inputMargin
	// lets us use a fast path for emitLiteral in the main loop, while we are
	// looking for copies.
	sLimit := len(src) - inputMargin
	if len(src) < minNonLiteralBlockSize {
		return 0
	}

	// Initialize the hash tables.
	const (
		// Long hash matches.
		lTableBits    = 17
		maxLTableSize = 1 << lTableBits

		// Short hash matches.
		sTableBits    = 14
		maxSTableSize = 1 << sTableBits
	)

	var lTable *[maxLTableSize]uint32
	if t := encLPool.Get(); t != nil {
		lTable = t.(*[maxLTableSize]uint32)
		*lTable = [maxLTableSize]uint32{}
	} else {
		lTable = new([maxLTableSize]uint32)
	}
	defer encLPool.Put(lTable)
	var sTable [maxSTableSize]uint32

	// Bail if we can't compress to at least this.
	dstLimit := len(src) - len(src)>>5 - 6

	// nextEmit is where in src the next emitLiteral should start from.
	nextEmit := 0

	// The encoded form must start with a literal, as there are no previous
	// bytes to copy, so we start looking for hash matches at s == 1.
	s := 1
	cv := load64(src, s)

	// We initialize repeat to 0, so we never match on first attempt
	repeat := 1
	lastWasRepeat := false

	if debugEncode {
		fmt.Println("encodeBlockBetterGo: Starting encode")
	}

	for {
		candidateL := 0
		nextS := 0
		for {
			// Next src position to check
			nextS = s + (s-nextEmit)>>7 + 1
			if nextS > sLimit {
				goto emitRemainder
			}
			minSrcPos := s - maxCopy3Offset + 1
			hashL := hash7(cv, lTableBits)
			hashS := hash4(cv, sTableBits)
			candidateL = int(lTable[hashL])
			candidateS := int(sTable[hashS])
			lTable[hashL] = uint32(s)
			sTable[hashS] = uint32(s)

			valLong := load64(src, candidateL)
			valShort := load64(src, candidateS)

			// If long matches at least 8 bytes, use that.
			if candidateL > minSrcPos && cv == valLong {
				break
			}

			// Check repeat at offset checkRep.
			const checkRep = 1
			// Minimum length of a repeat. Tested with various values.
			const wantRepeatBytes = 4
			const repeatMask = ((1 << (wantRepeatBytes * 8)) - 1) << (8 * checkRep)
			if repeat > 0 && cv&repeatMask == load64(src, s-repeat)&repeatMask {
				base := s + checkRep
				// Extend back
				for i := base - repeat; base > nextEmit && i > 0 && src[i-1] == src[base-1]; {
					i--
					base--
				}
				// Bail if we exceed the maximum size.
				if d+(base-nextEmit) > dstLimit {
					return 0
				}

				// Extend forward
				candidate := s - repeat + wantRepeatBytes + checkRep
				s += wantRepeatBytes + checkRep
				for s < len(src) {
					if len(src)-s < 8 {
						if src[s] == src[candidate] {
							s++
							candidate++
							continue
						}
						break
					}
					if diff := load64(src, s) ^ load64(src, candidate); diff != 0 {
						s += bits.TrailingZeros64(diff) >> 3
						break
					}
					s += 8
					candidate += 8
				}

				nLits := base - nextEmit
				matchLen := s - base
				if lastWasRepeat && nLits >= 1 && nLits <= 2 && matchLen >= 4 && matchLen <= 11 {
					d += emitRepeatLits(dst[d:], src[nextEmit:base], matchLen)
					lastWasRepeat = true
				} else if lastWasRepeat && nLits == 0 {
					d += emitCopy(dst[d:], repeat, matchLen)
					lastWasRepeat = (repeat <= maxCopy1Offset && matchLen >= 274)
				} else {
					d += emitLiteral(dst[d:], src[nextEmit:base])
					// same as `add := emitCopy(dst[d:], repeat, s-base)` but skips storing offset.
					d += emitRepeat(dst[d:], matchLen)
					lastWasRepeat = true
				}
				nextEmit = s
				if s >= sLimit {
					goto emitRemainder
				}
				// Index in-between
				index0 := base + 1
				index1 := s - 2

				for index0 < index1 {
					cv0 := load64(src, index0)
					cv1 := load64(src, index1)
					lTable[hash7(cv0, lTableBits)] = uint32(index0)
					sTable[hash4(cv0>>8, sTableBits)] = uint32(index0 + 1)

					lTable[hash7(cv1, lTableBits)] = uint32(index1)
					sTable[hash4(cv1>>8, sTableBits)] = uint32(index1 + 1)
					index0 += 2
					index1 -= 2
				}

				cv = load64(src, s)
				continue
			}

			// Long likely matches 7, so take that.
			if candidateL >= minSrcPos && uint32(cv) == uint32(valLong) {
				break
			}

			// Check our short candidate
			if candidateS >= minSrcPos && uint32(cv) == uint32(valShort) {
				// Try a long candidate at s+1
				hashL = hash7(cv>>8, lTableBits)
				candidateL = int(lTable[hashL])
				lTable[hashL] = uint32(s + 1)
				if candidateL > minSrcPos && uint32(cv>>8) == load32(src, candidateL) {
					s++
					break
				}
				// Use our short candidate.
				candidateL = candidateS
				break
			}

			cv = load64(src, nextS)
			s = nextS
		}

		// Extend backwards
		for candidateL > 0 && s > nextEmit && src[candidateL-1] == src[s-1] {
			candidateL--
			s--
		}

		// Bail if we exceed the maximum size.
		if d+(s-nextEmit) > dstLimit {
			return 0
		}

		base := s
		offset := base - candidateL

		// Extend the 4-byte match as long as possible.
		s += 4
		candidateL += 4
		for s < len(src) {
			if len(src)-s < 8 {
				if src[s] == src[candidateL] {
					s++
					candidateL++
					continue
				}
				break
			}
			if diff := load64(src, s) ^ load64(src, candidateL); diff != 0 {
				s += bits.TrailingZeros64(diff) >> 3
				break
			}
			s += 8
			candidateL += 8
		}

		// Bail if the match is equal or worse to the encoding.
		if offset > 65535 && s-base <= 4 && repeat != offset {
			s = nextS + 1
			if s >= sLimit {
				goto emitRemainder
			}
			cv = load64(src, s)
			continue
		}

		lits := src[nextEmit:base]
		if len(lits) > 0 {
			if offset <= maxCopy2Offset {
				// 2 byte offsets.
				// In rare cases, literal + copy1 will be smaller, but
				// this is faster to decode and it is rare, so we accept that.
				if len(lits) > maxCopy2Lits || offset < 64 {
					d += emitLiteral(dst[d:], lits)
					d += emitCopy(dst[d:], offset, s-base)
					lastWasRepeat = (offset <= maxCopy1Offset && s-base >= 274)
				} else {
					d += emitCopyLits2(dst[d:], lits, offset, s-base)
					lastWasRepeat = (s-base > copy2LitMaxLen)
				}
			} else {
				// 3 byte offset
				if len(lits) > maxCopy3Lits {
					d += emitLiteral(dst[d:], lits)
					d += emitCopy(dst[d:], offset, s-base)
					lastWasRepeat = (offset <= maxCopy1Offset && s-base >= 274)
				} else {
					d += emitCopyLits3(dst[d:], lits, offset, s-base)
					lastWasRepeat = false
				}
			}
		} else {
			d += emitCopy(dst[d:], offset, s-base)
			lastWasRepeat = (offset <= maxCopy1Offset && s-base >= 274)
		}
		repeat = offset

		nextEmit = s
		if s >= sLimit {
			goto emitRemainder
		}

		if d > dstLimit {
			// Do we have space for more, if not bail.
			return 0
		}

		// Index short & long
		index0 := base + 1
		index1 := s - 2

		cv0 := load64(src, index0)
		cv1 := load64(src, index1)
		lTable[hash7(cv0, lTableBits)] = uint32(index0)
		sTable[hash4(cv0>>8, sTableBits)] = uint32(index0 + 1)

		// lTable could be postponed, but very minor difference.
		lTable[hash7(cv1, lTableBits)] = uint32(index1)
		sTable[hash4(cv1>>8, sTableBits)] = uint32(index1 + 1)
		index0 += 1
		index1 -= 1
		cv = load64(src, s)

		// Index large values sparsely in between.
		// We do two starting from different offsets for speed.
		index2 := (index0 + index1 + 1) >> 1
		for index2 < index1 {
			lTable[hash7(load64(src, index0), lTableBits)] = uint32(index0)
			lTable[hash7(load64(src, index2), lTableBits)] = uint32(index2)
			index0 += 2
			index2 += 2
		}
	}

emitRemainder:
	if nextEmit < len(src) {
		// Bail if we exceed the maximum size.
		if d+len(src)-nextEmit > dstLimit {
			return 0
		}
		d += emitLiteral(dst[d:], src[nextEmit:])
	}
	return d
}

var encLPool64K sync.Pool

// encodeBlockBetterGo64K is a specialized version that handles inputs <= 64KB
func encodeBlockBetterGo64K(dst, src []byte) (d int) {
	// sLimit is when to stop looking for offset/length copies. The inputMargin
	// lets us use a fast path for emitLiteral in the main loop, while we are
	// looking for copies.
	sLimit := len(src) - inputMargin

	// Initialize the hash tables.
	const (
		// Long hash matches.
		lTableBits    = 15
		maxLTableSize = 1 << lTableBits

		// Short hash matches.
		sTableBits    = 12
		maxSTableSize = 1 << sTableBits
	)

	var lTable *[maxLTableSize]uint16
	if t := encLPool64K.Get(); t != nil {
		lTable = t.(*[maxLTableSize]uint16)
		*lTable = [maxLTableSize]uint16{}
	} else {
		lTable = new([maxLTableSize]uint16)
	}
	defer encLPool64K.Put(lTable)
	var sTable [maxSTableSize]uint16

	// Bail if we can't compress to at least this.
	dstLimit := len(src) - len(src)>>5 - 6

	// nextEmit is where in src the next emitLiteral should start from.
	nextEmit := 0

	// The encoded form must start with a literal, as there are no previous
	// bytes to copy, so we start looking for hash matches at s == 1.
	s := 1
	cv := load64(src, s)

	// We initialize repeat to 0, so we never match on first attempt
	repeat := 1
	lastWasRepeat := false

	if debugEncode {
		fmt.Println("encodeBlockBetterGo64K: Starting encode")
	}

	for {
		candidateL := 0
		nextS := 0
		for {
			// Next src position to check
			nextS = s + (s-nextEmit)>>7 + 1
			if nextS > sLimit {
				goto emitRemainder
			}
			hashL := hash6(cv, lTableBits)
			hashS := hash4(cv, sTableBits)
			candidateL = int(lTable[hashL])
			candidateS := int(sTable[hashS])
			lTable[hashL] = uint16(s)
			sTable[hashS] = uint16(s)

			valLong := load64(src, candidateL)
			valShort := load64(src, candidateS)

			// If long matches at least 8 bytes, use that.
			if cv == valLong {
				break
			}

			// Check repeat at offset checkRep.
			const checkRep = 1
			// Minimum length of a repeat. Tested with various values.
			const wantRepeatBytes = 4
			const repeatMask = ((1 << (wantRepeatBytes * 8)) - 1) << (8 * checkRep)
			if repeat > 0 && cv&repeatMask == load64(src, s-repeat)&repeatMask {
				base := s + checkRep
				// Extend back
				for i := base - repeat; base > nextEmit && i > 0 && src[i-1] == src[base-1]; {
					i--
					base--
				}
				// Bail if we exceed the maximum size.
				if d+(base-nextEmit) > dstLimit {
					return 0
				}

				// Extend forward
				candidate := s - repeat + wantRepeatBytes + checkRep
				s += wantRepeatBytes + checkRep
				for s < len(src) {
					if len(src)-s < 8 {
						if src[s] == src[candidate] {
							s++
							candidate++
							continue
						}
						break
					}
					if diff := load64(src, s) ^ load64(src, candidate); diff != 0 {
						s += bits.TrailingZeros64(diff) >> 3
						break
					}
					s += 8
					candidate += 8
				}

				nLits := base - nextEmit
				matchLen := s - base
				if lastWasRepeat && nLits >= 1 && nLits <= 2 && matchLen >= 4 && matchLen <= 11 {
					d += emitRepeatLits(dst[d:], src[nextEmit:base], matchLen)
					lastWasRepeat = true
				} else if lastWasRepeat && nLits == 0 {
					d += emitCopy(dst[d:], repeat, matchLen)
					lastWasRepeat = (repeat <= maxCopy1Offset && matchLen >= 274)
				} else {
					d += emitLiteral(dst[d:], src[nextEmit:base])
					// same as `add := emitCopy(dst[d:], repeat, s-base)` but skips storing offset.
					d += emitRepeat(dst[d:], matchLen)
					lastWasRepeat = true
				}
				nextEmit = s
				if s >= sLimit {
					goto emitRemainder
				}
				// Index in-between
				index0 := base + 1
				index1 := s - 2

				for index0 < index1 {
					cv0 := load64(src, index0)
					cv1 := load64(src, index1)
					lTable[hash6(cv0, lTableBits)] = uint16(index0)
					sTable[hash4(cv0>>8, sTableBits)] = uint16(index0 + 1)

					lTable[hash6(cv1, lTableBits)] = uint16(index1)
					sTable[hash4(cv1>>8, sTableBits)] = uint16(index1 + 1)
					index0 += 2
					index1 -= 2
				}

				cv = load64(src, s)
				continue
			}

			// Long likely matches 7, so take that.
			if uint32(cv) == uint32(valLong) {
				break
			}

			// Check our short candidate
			if uint32(cv) == uint32(valShort) {
				// Try a long candidate at s+1
				hashL = hash6(cv>>8, lTableBits)
				candidateL = int(lTable[hashL])
				lTable[hashL] = uint16(s + 1)
				if uint32(cv>>8) == load32(src, candidateL) {
					s++
					break
				}
				// Use our short candidate.
				candidateL = candidateS
				break
			}

			cv = load64(src, nextS)
			s = nextS
		}

		// Extend backwards
		for candidateL > 0 && s > nextEmit && src[candidateL-1] == src[s-1] {
			candidateL--
			s--
		}

		// Bail if we exceed the maximum size.
		if d+(s-nextEmit) > dstLimit {
			return 0
		}

		base := s
		offset := base - candidateL

		// Extend the 4-byte match as long as possible.
		s += 4
		candidateL += 4
		for s < len(src) {
			if len(src)-s < 8 {
				if src[s] == src[candidateL] {
					s++
					candidateL++
					continue
				}
				break
			}
			if diff := load64(src, s) ^ load64(src, candidateL); diff != 0 {
				s += bits.TrailingZeros64(diff) >> 3
				break
			}
			s += 8
			candidateL += 8
		}

		lits := src[nextEmit:base]
		if len(lits) > 0 {
			// 2 byte offsets.
			// In rare cases, literal + copy1 will be smaller, but
			// this is faster to decode and it is rare, so we accept that.
			if len(lits) > maxCopy2Lits || offset < 64 {
				d += emitLiteral(dst[d:], lits)
				d += emitCopy(dst[d:], offset, s-base)
				lastWasRepeat = (offset <= maxCopy1Offset && s-base >= 274)
			} else {
				d += emitCopyLits2(dst[d:], lits, offset, s-base)
				lastWasRepeat = (s-base > copy2LitMaxLen)
			}
		} else {
			d += emitCopy(dst[d:], offset, s-base)
			lastWasRepeat = (offset <= maxCopy1Offset && s-base >= 274)
		}
		repeat = offset

		nextEmit = s
		if s >= sLimit {
			goto emitRemainder
		}

		if d > dstLimit {
			// Do we have space for more, if not bail.
			return 0
		}

		// Index short & long
		index0 := base + 1
		index1 := s - 2

		cv0 := load64(src, index0)
		cv1 := load64(src, index1)
		lTable[hash6(cv0, lTableBits)] = uint16(index0)
		sTable[hash4(cv0>>8, sTableBits)] = uint16(index0 + 1)

		// lTable could be postponed, but very minor difference.
		lTable[hash6(cv1, lTableBits)] = uint16(index1)
		sTable[hash4(cv1>>8, sTableBits)] = uint16(index1 + 1)
		index0 += 1
		index1 -= 1
		cv = load64(src, s)

		// Index large values sparsely in between.
		// We do two starting from different offsets for speed.
		index2 := (index0 + index1 + 1) >> 1
		for index2 < index1 {
			lTable[hash6(load64(src, index0), lTableBits)] = uint16(index0)
			lTable[hash6(load64(src, index2), lTableBits)] = uint16(index2)
			index0 += 2
			index2 += 2
		}
	}

emitRemainder:
	if nextEmit < len(src) {
		// Bail if we exceed the maximum size.
		if d+len(src)-nextEmit > dstLimit {
			return 0
		}
		d += emitLiteral(dst[d:], src[nextEmit:])
	}
	return d
}
