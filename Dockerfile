FROM espressif/idf@sha256:6b4adab7b282e9261a154d7130fb4945a3c28bd35c2e914357c2f522643cb92a

ARG ESP_IDF_COMMIT=3ad36321ea7e6183986e19c7e05ee03052d181c3
ARG RUST_VERSION=1.97.0
ARG ESPUP_VERSION=0.15.0
ARG ESP_RUST_TOOLCHAIN_VERSION=1.97.0.0
ARG ESPFLASH_VERSION=4.0.1

ENV IDF_PATH=/opt/esp/idf
ENV CARGO_HOME=/opt/esp/cargo
ENV RUSTUP_HOME=/opt/esp/rustup
ENV PATH=/opt/esp/cargo/bin:/opt/esp/rustup/toolchains/esp/bin:${PATH}

USER root

RUN set -eux; \
    git -C "${IDF_PATH}" fetch --depth 1 origin "${ESP_IDF_COMMIT}"; \
    git -C "${IDF_PATH}" checkout --detach "${ESP_IDF_COMMIT}"; \
    git -C "${IDF_PATH}" submodule update --init --recursive --depth 1; \
    "${IDF_PATH}/install.sh" esp32s3; \
    test "$(git -C "${IDF_PATH}" rev-parse HEAD)" = "${ESP_IDF_COMMIT}"; \
    test -z "$(git -C "${IDF_PATH}" status --porcelain)"; \
    curl --proto '=https' --tlsv1.2 --fail --location https://sh.rustup.rs -o /tmp/rustup-init.sh; \
    sh /tmp/rustup-init.sh -y --profile minimal --default-toolchain "${RUST_VERSION}"; \
    rm /tmp/rustup-init.sh; \
    cargo install --locked espup --version "${ESPUP_VERSION}"; \
    espup install --targets esp32s3 --toolchain-version "${ESP_RUST_TOOLCHAIN_VERSION}" --export-file /opt/esp/export-esp.sh; \
    cargo install --locked espflash --version "${ESPFLASH_VERSION}"; \
    cargo --version; \
    espflash --version; \
    rm -rf "${CARGO_HOME}/registry" "${CARGO_HOME}/git"

WORKDIR /workspace
