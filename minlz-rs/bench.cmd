@echo off
echo MinLZ Rust Benchmarks
echo.
echo 1. Quick test (random data benchmark)
echo 2. All benchmarks (SLOW - takes a long time!)
echo 3. Level 1 benchmarks only
echo 4. Level 2 benchmarks only
echo 5. Twain series benchmarks
echo 6. File-based benchmarks
echo 7. Run tests only
echo.
choice /c 1234567 /m "Select option: "

if %errorlevel%==1 (
    echo.
    echo Running quick random data benchmark...
    cargo bench --bench simple_benchmarks
    goto end
)
if %errorlevel%==2 (
    echo.
    echo Running all benchmarks - this will take a long time...
    echo Press Ctrl+C to cancel if needed.
    timeout /t 3 /nobreak
    cargo bench
    goto end
)
if %errorlevel%==3 (
    echo.
    echo Running Level 1 benchmarks...
    cargo bench --bench comparison_benchmarks -- "level-1"
    goto end
)
if %errorlevel%==4 (
    echo.
    echo Running Level 2 benchmarks...
    cargo bench --bench comparison_benchmarks -- "level-2"
    goto end
)
if %errorlevel%==5 (
    echo.
    echo Running Twain series benchmarks...
    cargo bench --bench comparison_benchmarks -- "twain"
    goto end
)
if %errorlevel%==6 (
    echo.
    echo Running file-based benchmarks...
    cargo bench --bench comparison_benchmarks -- "encode_html"
    goto end
)
if %errorlevel%==7 (
    echo.
    echo Running test suite...
    cargo test
    goto end
)

:end
echo.
echo Benchmark completed!
echo HTML reports available in: target\criterion\
echo For more options, see BENCHMARKS.md