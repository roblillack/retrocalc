# Build and install RetroCalc from a checkout:
#
#     make
#     sudo make install
#
# PREFIX defaults to /usr/local; DESTDIR stages the install for packaging:
#
#     make install PREFIX=/usr DESTDIR=pkg
#
# `make install` doesn't build, so that `sudo` never runs cargo and leaves a
# root-owned target/ behind.

PREFIX  ?= /usr/local
DESTDIR ?=
CARGO   ?= cargo

BINDIR  = $(PREFIX)/bin
DATADIR = $(PREFIX)/share
BIN     = target/release/retrocalc

.PHONY: all build install uninstall clean

all: build

build:
	$(CARGO) build --release $(CARGOFLAGS)

install:
	@test -x $(BIN) || { echo "$(BIN) is missing: run 'make' first." >&2; exit 1; }
	install -Dm755 $(BIN) $(DESTDIR)$(BINDIR)/retrocalc
	install -Dm644 contrib/retrocalc.desktop $(DESTDIR)$(DATADIR)/applications/retrocalc.desktop
	install -Dm644 assets/retrocalc.svg $(DESTDIR)$(DATADIR)/icons/hicolor/scalable/apps/retrocalc.svg
	@$(MAKE) --no-print-directory refresh

uninstall:
	rm -f $(DESTDIR)$(BINDIR)/retrocalc
	rm -f $(DESTDIR)$(DATADIR)/applications/retrocalc.desktop
	rm -f $(DESTDIR)$(DATADIR)/icons/hicolor/scalable/apps/retrocalc.svg
	@$(MAKE) --no-print-directory refresh

# Tell the desktop about the change, where the tools are about. A staged
# install (DESTDIR) leaves this to the package manager.
.PHONY: refresh
refresh:
	@:
ifeq ($(DESTDIR),)
	-@command -v update-desktop-database >/dev/null && update-desktop-database -q $(DATADIR)/applications || true
	-@command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -qtf $(DATADIR)/icons/hicolor || true
endif

clean:
	$(CARGO) clean
