#!/bin/sh
# zig-as-musl-cc wrapper. Set ZIG to the zig binary if it is not on PATH.
exec "${ZIG:-zig}" cc -target aarch64-linux-musl "$@"
