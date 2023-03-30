FROM rust:1.68.0
WORKDIR /app
COPY . .
RUN cargo install --path .
CMD ["andrena"]
