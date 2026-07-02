package main

import (
	"bytes"
	"flag"
	"fmt"
	"io"
	"os"
	"strings"
	"time"

	"github.com/minio/minlz"
	"github.com/minio/minlz/cmd/internal/filepathx"
)

func mainSearch(args []string) {
	var (
		fs = flag.NewFlagSet("search", flag.ExitOnError)

		count     = fs.Bool("c", false, "Only print count of matching blocks/lines")
		noColor   = fs.Bool("no-color", false, "Disable colored output")
		lineN     = fs.Bool("n", false, "Print match line numbers")
		bail      = fs.Bool("bail", false, "Return error if search tables cannot be used")
		quiet     = fs.Bool("q", false, "Quiet: only set exit code (0=found, 1=not found)")
		lines     = fs.Bool("l", true, "Print matching lines instead of whole blocks")
		verbose   = fs.Bool("v", false, "Print data")
		winStats  = fs.Bool("window-stats", false, "Print per-pattern-window table presence counts after the search (implies -v)")
		sidecar   = fs.String("sidecar", "", "Search using the given sidecar (.mzs) file; the input must support random access. If empty, <input>"+minlzSidecarExt+" is auto-detected when present.")
		noSidecar = fs.Bool("no-sidecar", false, "Disable sidecar auto-detection; force inline search")
		help      = fs.Bool("help", false, "Display help")
	)
	fs.Usage = func() {
		w := fs.Output()
		_, _ = fmt.Fprintln(w, `Search for a pattern in compressed MinLZ streams.

The pattern is a literal byte string (not a regex).

Options:`)
		fs.PrintDefaults()
		fmt.Fprintf(w, "\nUsage: %v search [options] <pattern> <input...>\n", os.Args[0])
	}
	fs.Parse(args)
	args = fs.Args()
	if *help || len(args) < 2 {
		fs.Usage()
		if *help {
			os.Exit(0)
		}
		os.Exit(1)
	}
	_ = noColor
	pattern := []byte(args[0])
	files := args[1:]
	verboseOut := *verbose || *winStats

	exitCode := 1 // 1 = not found
	multiFile := len(files) > 1

	for _, fileArg := range files {
		var matches []string
		var err error
		if strings.ContainsAny(fileArg, "*?[") {
			matches, err = filepathx.Glob(fileArg)
			exitErr(err)
		} else {
			matches = []string{fileArg}
		}

		for _, file := range matches {
			start := time.Now()
			found, stats, err := searchFile(file, pattern, searchOpts{
				count:     *count,
				lineNums:  *lineN,
				bail:      *bail,
				quiet:     *quiet,
				lines:     *lines,
				verbose:   verboseOut,
				multiFile: multiFile,
				sidecar:   *sidecar,
				noSidecar: *noSidecar,
			})
			if err != nil {
				fmt.Fprintf(os.Stderr, "%s: %v\n", file, err)
				continue
			}
			if found {
				exitCode = 0
			}
			if verboseOut {
				elapsed := time.Since(start).Round(time.Millisecond)
				mbps := float64(stats.UncompressedSize) / elapsed.Seconds() / 1e6
				fmt.Fprintf(os.Stderr, "%s took %v %.01f MB/s\n", file, elapsed, mbps)
				if *winStats {
					stats.FprintExtended(os.Stderr)
				} else {
					stats.Fprint(os.Stderr)
				}
			}
		}
	}
	if *quiet {
		os.Exit(exitCode)
	}
}

type searchOpts struct {
	count     bool
	lineNums  bool
	bail      bool
	quiet     bool
	lines     bool
	verbose   bool
	multiFile bool
	sidecar   string
	noSidecar bool
}

func searchFile(file string, pattern []byte, opts searchOpts) (found bool, stats minlz.SearchStats, err error) {
	// Auto-detect a sidecar at <file>.mzs unless one was explicitly given
	// or auto-detection was disabled.
	if opts.sidecar == "" && !opts.noSidecar && file != "-" {
		candidate := file + minlzSidecarExt
		if st, serr := os.Stat(candidate); serr == nil && !st.IsDir() {
			opts.sidecar = candidate
			if opts.verbose {
				fmt.Fprintf(os.Stderr, "%s: using sidecar %s\n", file, candidate)
			}
		}
	}

	var bsOpts []minlz.BlockSearchOption
	if opts.bail {
		bsOpts = append(bsOpts, minlz.BlockSearchBailOnMissing())
	}
	if opts.verbose {
		bsOpts = append(bsOpts, minlz.BlockSearchCollectStats())
		bsOpts = append(bsOpts, minlz.BlockSearchInfoCallback(func(cfg minlz.SearchTableConfig) {
			fmt.Fprintf(os.Stderr, "%s: search info: %s\n", file, cfg)
		}))
	}

	type genericSearcher interface {
		Search(pattern []byte, fn func(minlz.SearchResult) error) error
		Stats() minlz.SearchStats
	}
	var searcher genericSearcher

	if opts.sidecar != "" {
		// Sidecar search: main must support io.ReaderAt.
		if file == "-" {
			return false, stats, fmt.Errorf("sidecar search requires a seekable input, not stdin")
		}
		mainF, err := os.Open(file)
		if err != nil {
			return false, stats, err
		}
		defer mainF.Close()
		sideF, err := os.Open(opts.sidecar)
		if err != nil {
			return false, stats, err
		}
		defer sideF.Close()
		searcher = minlz.NewSidecarSearcher(mainF, sideF, bsOpts...)
	} else {
		var r io.Reader
		if file == "-" {
			r = os.Stdin
		} else {
			f, err := os.Open(file)
			if err != nil {
				return false, stats, err
			}
			defer f.Close()
			r = f
		}
		searcher = minlz.NewBlockSearcher(r, bsOpts...)
	}

	matchCount := 0
	lineOffset := int64(1)
	lastLineStart := int64(-1)

	err = searcher.Search(pattern, func(r minlz.SearchResult) error {
		found = true
		if opts.quiet {
			return fmt.Errorf("done")
		}

		prefix := ""
		if opts.multiFile {
			prefix = file + ":"
		}

		if opts.lines {
			// Count each matching line once. Dedup by the line's start offset —
			// found by scanning back, into the previous block only when the line
			// begins there — rather than by its end. This keeps a line that
			// straddles a block boundary and matches in both halves counted once,
			// matching grep/rg.
			ls := lineStartOffset(r)
			if ls == lastLineStart {
				return nil
			}
			lastLineStart = ls
			if opts.count {
				matchCount++
				return nil
			}
			line := extractLine(r, pattern)
			matchCount++
			if opts.lineNums {
				fmt.Printf("%s%d:%d:%s\n", prefix, lineOffset, r.StreamOffset, line)
			} else {
				fmt.Printf("%s%d:%s\n", prefix, r.StreamOffset, line)
			}
			lineOffset++
		} else {
			matchCount++
			if opts.count {
				return nil
			}
			fmt.Printf("%s%d:\n", prefix, r.StreamOffset)
		}
		return nil
	})
	if err != nil && err.Error() == "done" {
		err = nil
	}

	stats = searcher.Stats()
	if err != nil {
		return found, stats, err
	}
	if opts.count && !opts.quiet {
		prefix := ""
		if opts.multiFile {
			prefix = file + ": "
		}
		fmt.Printf("%s%d\n", prefix, matchCount)
	}
	return found, stats, nil
}

// lineStartOffset returns the absolute stream offset of the start of the line
// containing the match. It scans the current block back to the preceding
// newline and only consults the previous block (via PrevBlock, which may lazily
// decode) when the line begins before the current block — so counting a line
// that lies within one block never touches the previous block.
func lineStartOffset(r minlz.SearchResult) int64 {
	pl := r.PrevBlockLen
	if posInCur := r.Offset - pl; posInCur > 0 {
		if nl := bytes.LastIndexByte(r.Blocks[1][:posInCur], '\n'); nl >= 0 {
			return r.BlockStart + int64(pl+nl+1)
		}
	}
	prev := r.PrevBlock()
	if end := min(r.Offset, len(prev)); end > 0 {
		if nl := bytes.LastIndexByte(prev[:end], '\n'); nl >= 0 {
			return r.BlockStart + int64(nl+1)
		}
	}
	return r.BlockStart
}

// extractLine returns the line containing the match. It copies only the line's
// bytes (never whole blocks): a sub-slice of one block when the line fits in it,
// or the previous block's tail joined with the current block's head when the
// line straddles the boundary. The line is truncated at the current block's end
// if it continues into the next block (no forward block is fetched).
func extractLine(r minlz.SearchResult, pattern []byte) string {
	prev := r.PrevBlock()
	cur := r.Blocks[1]
	pl := len(prev)

	start := 0
	if nl := lastNewline(prev, cur, r.Offset); nl >= 0 {
		start = nl + 1
	}
	end := pl + len(cur)
	if nl := firstNewline(prev, cur, r.Offset+len(pattern)); nl >= 0 {
		end = nl
	}

	switch {
	case end <= pl:
		return string(prev[start:end])
	case start >= pl:
		return string(cur[start-pl : end-pl])
	default:
		buf := make([]byte, 0, end-start)
		buf = append(buf, prev[start:]...)
		buf = append(buf, cur[:end-pl]...)
		return string(buf)
	}
}

// lastNewline returns the index of the last '\n' strictly before upto in the
// logical buffer prev||cur, or -1. firstNewline returns the index of the first
// '\n' at or after from. Both index the concatenation without materializing it.
func lastNewline(prev, cur []byte, upto int) int {
	pl := len(prev)
	if upto > pl {
		if i := bytes.LastIndexByte(cur[:upto-pl], '\n'); i >= 0 {
			return pl + i
		}
		upto = pl
	}
	if upto > 0 {
		return bytes.LastIndexByte(prev[:min(upto, pl)], '\n')
	}
	return -1
}

func firstNewline(prev, cur []byte, from int) int {
	pl := len(prev)
	if from < 0 {
		from = 0
	}
	if from < pl {
		if i := bytes.IndexByte(prev[from:], '\n'); i >= 0 {
			return from + i
		}
		from = pl
	}
	if from-pl < len(cur) {
		if i := bytes.IndexByte(cur[from-pl:], '\n'); i >= 0 {
			return from + i
		}
	}
	return -1
}
