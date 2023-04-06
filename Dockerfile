FROM rust:1.68.0
RUN apt-get update && apt-get install -y cmake
WORKDIR /app
COPY . .
RUN cargo install --path .
CMD ["andrena"]
