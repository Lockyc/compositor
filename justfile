# compositor — task runner

# `default` pipes `just --list` through a small stock-perl filter that clips long recipe
# docs to your terminal width (…) instead of wrapping. Self-contained — no external files;
# falls back to plain `just --list` where perl is absent. Edit the recipes below, not this.
# List available recipes
default:
    @if command -v perl >/dev/null 2>&1; then just --color always --list | perl -CS -Mutf8 -lpe 'BEGIN{($w)=`stty size 2>/dev/null </dev/tty`=~/ (\d+)/; $w||=100; $col=(-t STDOUT && !exists $ENV{NO_COLOR})} s/\e\[[0-9;]*m//g unless $col; (my $v=$_)=~s/\e\[[0-9;]*m//g; if(length($v)>$w){my($o,$n)=("",0); while(length && $n<$w-1){ if($col && s/^(\e\[[0-9;]*m)//){$o.=$1}else{s/^(.)//;$o.=$1;$n++} } $_=$o."…".($col?"\e[0m":"")}'; else just --list; fi

# Build the workspace
[group("build")]
build:
    cargo build

# Run the CLI (e.g. `just run build --dir .`)
[group("build")]
run *args:
    cargo run -p compositor -- {{args}}

# Run the test suite
[group("check")]
test:
    cargo test

# Compile without producing binaries
[group("check")]
check:
    cargo check --all-targets

# Format all Rust files in place
[group("check")]
fmt:
    cargo fmt

# Clippy lints (warnings are errors)
[group("check")]
lint:
    cargo clippy --all-targets -- -D warnings

# Non-mutating pre-merge gate: fmt check + clippy + tests
[group("check")]
gate:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo fmt --check
    cargo clippy --all-targets -- -D warnings
    cargo test
    echo "✓ gate passed"

# Cut a release: bump the workspace version + write RELEASE_NOTES.md first, then run this
[group("release")]
release notes="RELEASE_NOTES.md":
    #!/usr/bin/env bash
    set -euo pipefail
    version="$(grep -m1 '^version = ' Cargo.toml | cut -d'"' -f2)"
    tag="v${version}"
    # Check the notes FIRST: everything below this point (tag, push, release) is
    # public and awkward to retract, so a missing notes file must fail before any
    # of it happens, not between the tag push and the release create.
    if [ ! -s "{{notes}}" ]; then
      echo "✗ no release notes at '{{notes}}' — write them, then re-run." >&2
      echo "  Summarise what shipped since the previous tag (features / fixes / docs)," >&2
      echo "  leading with anything that changes what a consuming site renders." >&2
      echo "  Pass a different path with: just release <file>" >&2
      exit 1
    fi
    if [ -n "$(git status --porcelain)" ]; then
      echo "✗ working tree is dirty — commit the version bump first" >&2
      exit 1
    fi
    if git rev-parse -q --verify "refs/tags/${tag}" >/dev/null; then
      echo "✗ tag ${tag} already exists — bump the workspace version before releasing" >&2
      exit 1
    fi
    just gate
    git push origin dev
    # main only ever fast-forwards to a release commit. Assert it hasn't diverged
    # (the doc carve-out lands commits on main; one never merged back into dev would
    # be silently dropped by `branch -f`) so we fail loud instead of losing it.
    if ! git merge-base --is-ancestor main dev; then
      echo "✗ main is not an ancestor of dev — it diverged." >&2
      echo "  Run: git checkout dev && git merge main   (then re-run the release)." >&2
      exit 1
    fi
    git branch -f main dev
    git push origin main
    git tag -a "${tag}" -m "${tag}" main
    git push origin "${tag}"
    gh release create "${tag}" --target main --title "${tag}" --notes-file "{{notes}}"
    # release.yml builds the Linux binary on `release: published` and uploads it onto
    # this release — a Mac can't build x86_64-unknown-linux-gnu without cross tooling,
    # so the asset cannot be attached here. Wait for it rather than trusting it: a
    # release whose asset never lands strands every build host on the old binary, and
    # the tag looks successful either way. This wait IS the release, not a formality.
    echo "waiting for release.yml to attach the Linux binary…"
    for _ in $(seq 1 60); do
      if [ "$(gh release view "${tag}" --json assets --jq '.assets | length')" -ge 2 ]; then
        break
      fi
      sleep 10
    done
    if [ "$(gh release view "${tag}" --json assets --jq '.assets | length')" -lt 2 ]; then
      echo "✗ ${tag} published but its assets never arrived — build hosts would stay on the old binary." >&2
      echo "  Check:  gh run list --workflow=release.yml" >&2
      echo "  No run at all? Actions has gone deaf — see CLAUDE.md § Branching & releases." >&2
      echo "  Recover: gh workflow run release.yml -f tag=${tag}" >&2
      exit 1
    fi
    echo "✓ released ${tag} with both assets"
