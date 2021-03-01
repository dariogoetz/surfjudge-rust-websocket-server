FROM rust:latest as builder

RUN USER=root cargo new --bin websockets-async
WORKDIR ./websockets-async

RUN apt-get update \
  && apt-get install -y libzmq3-dev \
  && rm -rf /var/lib/apt/lists/*

COPY ./Cargo.toml   ./Cargo.toml

RUN cargo build --release
RUN rm src/*.rs

ADD . ./

RUN rm ./target/release/deps/websockets_async*
RUN cargo build --release


FROM debian:buster-slim
ARG APP=/usr/src/app

RUN apt-get update \
  && apt-get install -y libzmq3-dev \
  && rm -rf /var/lib/apt/lists/*

ENV APP_USER=appuser
RUN groupadd $APP_USER \
    && useradd -g $APP_USER $APP_USER \
    && mkdir -p ${APP}

RUN chown -R $APP_USER:$APP_USER ${APP}
USER $APP_USER
WORKDIR ${APP}

COPY --from=builder /websockets-async/target/release/websockets-async ${APP}/websockets-async

CMD ["./websockets-async"]
