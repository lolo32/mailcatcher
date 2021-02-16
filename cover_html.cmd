@echo Installing Grcov
@rem cargo install grcov

@echo Clean previous
del *.profraw
rd /s /q target_cov\coverage

@echo Setting general env
set RUST_LOG=mailcatcher
set CARGO_TARGET_DIR=target_cov
set RUST_TEST_TIME_UNIT=150,500


@echo Build binary
rem set RUSTFLAGS=-Zinstrument-coverage
set CARGO_INCREMENTAL=0
set RUSTFLAGS=-Zprofile -Ccodegen-units=1 -Copt-level=0 -Clink-dead-code -Coverflow-checks=off
cargo build --all-features

@echo Build and run tests
set LLVM_PROFILE_FILE=your_name-%p-%m.profraw
cargo test --all-features --color=always -- -Z unstable-options --show-output --report-time=colored
set LLVM_PROFILE_FILE=

@echo Generate coverage
grcov target_cov -t html --llvm --branch --ignore-not-existing -o target_cov\coverage
@rem grcov . --source-dir . --llvm --binary-path .\target_cov\debug\ --branch --ignore-not-existing --ignore "C:\*" --output-type html --output-path .\target_cov\debug\coverage
