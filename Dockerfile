FROM rust:bookworm as build
COPY ./src ./src
COPY ./Cargo.lock .
COPY ./Cargo.toml .

# RUN adduser \
#     --disabled-password \
#     --gecos "" \
#     --home "/nonexistent" \
#     --shell "/sbin/nologin" \
#     --no-create-home \
#     --uid 10001 \
#     "apiuser"

RUN cargo build --release

FROM debian:bookworm-slim
# COPY --from=build /etc/passwd /etc/passwd
# COPY --from=build /etc/group /etc/group

# USER apiuser:apiuser

RUN apt-get update && \
  apt-get install -y \
  libsqlite3-0 \
  libssl3 \
  && \
  apt-get autoremove -y && \
  apt-get clean -y && \
  rm -rf /var/lib/apt/lists/*

COPY --from=build ./target/release/stuwe-telegram-rs /app/stuwe-telegram-rs
WORKDIR /app/data
ENTRYPOINT ["/app/stuwe-telegram-rs"]