# Makefile — Build libflux_rt.a (Flux minimal C runtime)
#
# Usage:
#   make              Build libflux_rt.a (static library)
#   make shared       Build libflux_rt.so / libflux_rt.dylib
#   make clean        Remove build artifacts
#   make test         Build and run the smoke test
#
# Variables:
#   CC          C compiler (default: cc)
#   CFLAGS      Additional compiler flags
#   AR          Archiver (default: ar)

CC      ?= cc
AR      ?= ar
CFLAGS  += -std=c11 -Wall -Wextra -Wpedantic -O2 -g
LDFLAGS ?=

SRCS    = gc.c flux_rt.c string.c hamt.c effects.c array.c
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
	rm -f $(OBJS) $(LIB) $(SHARED_LIB) smoke_test smoke_test.o

# ── Smoke test ───────────────────────────────────────────────────────
#
# Compile a minimal program that exercises the runtime.
# The test provides its own main() so we compile flux_rt.c without
# the default main() by defining FLUX_RT_NO_MAIN.

test: $(LIB) smoke_test.c
	$(CC) $(CFLAGS) -DFLUX_RT_NO_MAIN -c -o flux_rt_nomain.o flux_rt.c
	$(AR) rcs libflux_rt_nomain.a gc.o flux_rt_nomain.o string.o hamt.o effects.o
	$(CC) $(CFLAGS) -o smoke_test smoke_test.c -L. -lflux_rt_nomain $(LDFLAGS)
	./smoke_test
	@echo "smoke test passed"
	@rm -f flux_rt_nomain.o libflux_rt_nomain.a smoke_test
