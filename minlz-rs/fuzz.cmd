@echo off
echo MinLZ Rust Fuzz Testing
echo.
echo 1. Run all fuzz tests (default proptest settings)
echo 2. Medium intensity (1,000 test cases per property)
echo 3. High intensity (10,000 test cases per property)
echo 4. Interactive continuous fuzzing (configurable iterations)
echo 5. Level 1 specific fuzz test
echo 6. Level 2 specific fuzz test
echo 7. Level 3 specific fuzz test
echo 8. Round-trip and edge case tests
echo 9. Continuous fuzzing module (high-intensity tests)
echo.
choice /c 123456789 /m "Select option"

if %errorlevel%==1 goto option1
if %errorlevel%==2 goto option2
if %errorlevel%==3 goto option3
if %errorlevel%==4 goto option4
if %errorlevel%==5 goto option5
if %errorlevel%==6 goto option6
if %errorlevel%==7 goto option7
if %errorlevel%==8 goto option8
if %errorlevel%==9 goto option9

:option1
echo.
echo Running all fuzz tests...
cargo test fuzz_tests:: -- --nocapture
goto end

:option2
echo.
echo Running medium intensity fuzz tests...
set PROPTEST_CASES=1000
cargo test fuzz_tests:: -- --nocapture
goto end

:option3
echo.
echo Running high intensity fuzz tests...
set PROPTEST_CASES=10000
cargo test fuzz_tests:: -- --nocapture
goto end

:option4
echo.
echo Interactive Continuous Fuzzing Configuration
echo.
set /p cases=Enter test cases per iteration (default 1000):
if "%cases%"=="" set cases=1000

set /p iterations=Enter number of iterations (0 for infinite):
if "%iterations%"=="" set iterations=0

echo.
echo Configuration:
echo - Test cases per iteration: %cases%
echo - Iterations: %iterations%
echo.
echo Starting continuous fuzzing...
echo Press Ctrl+C to stop at any time

set iteration_count=0
set PROPTEST_CASES=%cases%

:loop4
set /a iteration_count+=1
echo.
echo Iteration %iteration_count% - %date% %time%
echo.
cargo test fuzz_tests:: --release -- --nocapture

if errorlevel 1 (
    echo.
    echo FUZZ TEST FAILURE DETECTED!
    echo Press any key to continue or Ctrl+C to stop...
    pause >nul
)

if %iterations% neq 0 if %iteration_count% geq %iterations% goto end

echo Iteration %iteration_count% completed
ping 127.0.0.1 -n 3 >nul
goto loop4

:option5
echo.
echo Running Level 1 specific fuzz test...
cargo test test_level1_never_panics -- --nocapture
goto end

:option6
echo.
echo Running Level 2 specific fuzz test...
cargo test test_level2_never_panics -- --nocapture
goto end

:option7
echo.
echo Running Level 3 specific fuzz test...
cargo test test_level3_never_panics -- --nocapture
goto end

:option8
echo.
echo Running round-trip and edge case tests...
cargo test test_round_trip_property -- --nocapture
cargo test test_block_level_round_trip -- --nocapture
cargo test test_edge_pattern_data -- --nocapture
cargo test test_compression_consistency -- --nocapture
goto end

:option9
echo.
echo Running continuous fuzzing module...
cargo test continuous_fuzz:: --release -- --nocapture
goto end

:end
echo.
echo Fuzz testing completed!