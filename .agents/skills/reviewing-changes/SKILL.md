---
name: reviewing-changes
description: Use for a read-only review of ESP firmware changes, worktree state, secret exposure, documentation drift, or hardware mutation risk.
---

# Reviewing Changes

Use this guidance with the installed `making-changes` skill. It supplies repository-specific review inputs and does not own generic review steps.

Keep the review read-only and report findings first.

## Canonical Guidance

- [Repository agent index](../../../AGENTS.md)
- [Documentation index](../../../docs/README.md)
- [Firmware contract](../../../docs/firmware.md)

## Review Inputs

- Inspect the requested comparison base when the request supplies one.
- Inspect HEAD plus all staged, unstaged, and untracked worktree states.
- Read each changed file and its relevant source or documentation counterpart.
- Treat source and configuration as evidence of implementation, not proof of live device state.
- Distinguish observed defects from unverified hardware behavior.

## Review Focus

- Compare firmware behavior against `docs/firmware.md` and flag unsupported or stale claims.
- Check changed files, diffs, logs, and tracked artifacts for credential exposure.
- Check runner or command changes for implicit flash, monitor, erase, or other hardware mutation.
- Check target, toolchain, dependency, allocator, executor, radio, DHCP, and retry changes for coupled effects.
- Check that documentation excludes one-off device addresses, network names, and test outcomes.

## Validation Evidence

- Inspect evidence for `cargo fmt --check` and `cargo check --release` when the change affects Rust source.
- Require `SSID`, `PASSWORD`, and `HOSTNAME` for compile-check evidence.
- Do not describe an unrun or unavailable check as successful.
- Do not treat compilation as proof of Wi-Fi, DHCP, or device behavior.

## Guardrails

- Do not edit files, commit, push, flash hardware, access a device, or access an external system.
- Do not run `cargo run --release`, `espflash`, or any command that can mutate hardware.
- Do not expose credential values while you inspect changes or report findings.
- Require explicit authorization outside the review before any hardware mutation.

## Output

1. Report findings first, in severity order, with precise path references.
2. State that no findings exist when the review finds no actionable defect.
3. List validation evidence and unverified checks.
4. End with open questions or residual risks only when they remain.
