#!/bin/sh

echo Installing Grcov
cargo install grcov

echo Clean previous
rm *.profraw
rm -fr target/debug/coverage

echo Build binary
export RUSTFLAGS="-Zinstrument-coverage"
cargo build
echo Build and run tests
LLVM_PROFILE_FILE="your_name-%p-%m.profraw" cargo test

echo Generate coverage
grcov . \
    --source-dir . \
    --llvm \
    --binary-path ./target/debug/ \
    --branch \
    --ignore-not-existing \
    --ignore "/*" \
    --output-type html \
    --output-path ./target/debug/coverage
