# Makefile — Build libflux_rt.a (Flux minimal C runtime)
#
# Usage:
#   make              Build libflux_rt.a (static library)
#   make shared       Build libflux_rt.so / libflux_rt.dylib
#   make clean        Remove build artifacts
#   make test         Build and run the C runtime tests (through cargo)
#
# Variables:
#   CC          C compiler (default: cc)
#   CFLAGS      Additional compiler flags
#   AR          Archiver (default: ar)

CC      ?= cc
AR      ?= ar
CFLAGS  += -std=c11 -Wall -Wextra -Wpedantic -O2 -g -funwind-tables -fexceptions
LDFLAGS ?=

SRCS    = rc.c flux_rt.c string.c hamt.c effects.c array.c tasks.c channel.c tcp.c \
          sha256.c tape.c
OBJS    = $(SRCS:.c=.o)
LIB     = libflux_rt.a

# Detect OS for shared library extension.
UNAME_S := $(shell uname -s)
ifeq ($(UNAME_S),Darwin)
  SHARED_LIB = libflux_rt.dylib
  SHARED_FLAGS = -dynamiclib -install_name @rpath/$(SHARED_LIB)
else
  SHARED_LIB = libflux_rt.so
  SHARED_FLAGS = -shared -fPIC
endif

# pthreads must be linked explicitly on Linux (macOS includes it in -lSystem).
ifeq ($(UNAME_S),Linux)
  LDFLAGS += -lpthread
endif

# ── Targets ──────────────────────────────────────────────────────────

.PHONY: all shared clean test

all: $(LIB)

$(LIB): $(OBJS)
	$(AR) rcs $@ $^

shared: CFLAGS += -fPIC
shared: $(OBJS)
	$(CC) $(SHARED_FLAGS) -o $(SHARED_LIB) $(OBJS) $(LDFLAGS)

%.o: %.c flux_rt.h
	$(CC) $(CFLAGS) -c -o $@ $<

clean:
	rm -f $(OBJS) $(LIB) $(SHARED_LIB)

# ── C runtime tests ───────────────────────────────────────────────────────
#
# The tests in tests/ call into the Rust side of the runtime, so they are
# built and linked the way a native Flux program is, by
# tests/native_llvm/c_runtime_tests.rs. This target runs them.

test:
	cargo test --manifest-path ../../Cargo.toml --features llvm --test native_llvm c_runtime
