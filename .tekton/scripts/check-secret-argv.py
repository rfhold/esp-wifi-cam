#!/usr/bin/env python3
import pathlib
import sys


def main() -> int:
    targets = [
        pathlib.Path(".tekton/scripts/sign-release.sh"),
        pathlib.Path(".tekton/scripts/check-version-release.sh"),
        pathlib.Path(".tekton/scripts/check-channel-assets.sh"),
    ]
    forbidden = [
        "--" + "header",
        "FORGEJO" + "_TOKEN",
        "$(<" + '"$token_file")',
    ]
    for path in targets:
        text = path.read_text(encoding="ascii")
        for value in forbidden:
            if value in text:
                print(f"{path}: secret-bearing curl argument or token interpolation found", file=sys.stderr)
                return 1
        if text.count("curl --disable") != text.count('curl --disable --config "$'):
            print(f"{path}: every curl invocation must use a guarded config file", file=sys.stderr)
            return 1
    print("secret argv static validation passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
