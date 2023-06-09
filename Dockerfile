FROM rust:1.68 as builder
WORKDIR /usr/src/wintertodt_server
COPY . .
RUN cargo install --path .

FROM debian:buster-slim
COPY --from=builder /usr/local/cargo/bin/wintertodt_server /usr/local/bin/wintertodt_server

ENV WTWS_HOST="0.0.0.0:3000"

EXPOSE 3000/tcp

CMD wintertodt_server