.PHONY: build debug check test lint run install uninstall package package-deb package-appimage clean

export APPIMAGE_TOOL APPIMAGE_TOOL_SHA256

build:
	cargo build --release --locked

debug:
	cargo build --locked

check:
	cargo check --locked

test:
	cargo test --locked

lint:
	cargo fmt --check
	cargo clippy --locked --all-targets -- -D warnings

run:
	cargo run --locked

install: build
	scripts/install.sh --no-build

uninstall:
	scripts/uninstall.sh

package: build
	scripts/package.sh

package-deb: build
	bash scripts/package-deb.sh

# Use an independently verified local tool; never fetch/select trust implicitly.
package-appimage:
	@test -n "$${APPIMAGE_TOOL}" -a -n "$${APPIMAGE_TOOL_SHA256}" || { echo 'Set APPIMAGE_TOOL and APPIMAGE_TOOL_SHA256 from independently trusted metadata' >&2; exit 2; }
	bash scripts/package-appimage.sh --tool "$${APPIMAGE_TOOL}" --tool-sha256 "$${APPIMAGE_TOOL_SHA256}"

clean:
	cargo clean
	rm -rf dist
