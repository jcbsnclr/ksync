#!/bin/sh
# Compile every Typst document under docs/ into docs/build/.
#
# Usage: scripts/build-docs.sh [--watch]
#
# --watch rebuilds continuously for the first document only. Normal use
# compiles all documents once.

set -eu

ROOT=$(git rev-parse --show-toplevel 2>/dev/null) || {
    echo "build-docs: run this from inside the ksync repository" >&2
    exit 1
}

cd "$ROOT"

if ! command -v typst >/dev/null 2>&1; then
    echo "build-docs: typst is not installed" >&2
    exit 1
fi

OUT="$ROOT/docs/build"
mkdir -p "$OUT"

documents=$(find docs -type f -name '*.typ' | sort)

if [ -z "$documents" ]; then
    echo "build-docs: no .typ files under docs/" >&2
    exit 1
fi

case "${1:-}" in
    --watch)
        first=$(printf '%s\n' "$documents" | head -n 1)
        relative=${first#docs/}
        target="$OUT/${relative%.typ}.pdf"
        mkdir -p "$(dirname "$target")"
        echo "watching $first -> $target"
        exec typst watch --root "$ROOT" "$first" "$target"
        ;;
    '')
        ;;
    *)
        echo "usage: $0 [--watch]" >&2
        exit 2
        ;;
esac

printf '%s\n' "$documents" | while IFS= read -r file; do
    relative=${file#docs/}
    target="$OUT/${relative%.typ}.pdf"
    mkdir -p "$(dirname "$target")"
    echo "$file -> $target"
    typst compile --root "$ROOT" "$file" "$target"
done

echo "compiled to docs/build/"
