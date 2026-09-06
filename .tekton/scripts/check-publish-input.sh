#!/usr/bin/env bash
set -euo pipefail

export LC_ALL=C

directory=${1:?publish input directory is required}
shift
[[ -d $directory && ! -L $directory ]] || { printf '%s\n' 'publish input directory is invalid' >&2; exit 1; }

expected=$(mktemp)
actual=$(mktemp)
trap 'rm -f -- "$expected" "$actual"' EXIT
printf '%s\n' "$@" | sort > "$expected"
find "$directory" -mindepth 1 -maxdepth 1 -type f -printf '%f\n' | sort > "$actual"
cmp -s "$expected" "$actual" || {
  printf '%s\n' 'publish input file set does not match the release contract' >&2
  exit 1
}
while IFS= read -r name; do
  [[ -f $directory/$name && ! -L $directory/$name ]] || {
    printf '%s\n' "publish input is not a regular file: $name" >&2
    exit 1
  }
done < "$expected"
