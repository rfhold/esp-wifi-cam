# Rollback-enabled bootloader

The firmware requires a custom ESP-IDF second-stage bootloader built from commit `3ad36321ea7e6183986e19c7e05ee03052d181c3`. The repository intentionally does not contain a bootloader binary.

Set `IDF_PATH` to a clean ESP-IDF checkout at that exact commit, then run `scripts/build-bootloader.sh`. The script verifies the source revision, builds only the bootloader from the checked-in configuration, copies `bootloader.bin` into the ignored `bootloader/build/` directory, and records a SHA-256 file consumed by `scripts/flash.sh`.

The build input selects ESP32-S3, DIO, 40 MHz, 8 MiB flash, partition-table offset `0x8000`, reproducible application metadata, and application rollback support. Do not use a stock `espflash` bootloader: it does not provide the required rollback behavior.
