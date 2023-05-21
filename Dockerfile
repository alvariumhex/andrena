FROM rust:1.68.0
RUN apt-get update && apt-get install -y cmake

# yt-dlp
RUN curl -L https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_linux -o /usr/local/bin/yt-dlp
RUN chmod a+rx /usr/local/bin/yt-dlp

WORKDIR /app
COPY . .
RUN cargo install --path .
CMD ["andrena"]
