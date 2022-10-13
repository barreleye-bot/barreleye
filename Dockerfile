FROM rust:1.64 as build
RUN USER=root cargo new --bin barreleye-insights
WORKDIR /barreleye-insights
COPY ./ ./
RUN cargo build --release

FROM debian:bookworm
COPY --from=build /barreleye-insights/target/release/barreleye-insights .
CMD ["./barreleye-insights", "server", "--bannerless"]