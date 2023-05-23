FROM rust:1.68.0
RUN apt-get update && apt-get install -y cmake build-essential pkg-config wget unzip

# yt-dlp
RUN curl -L https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_linux -o /usr/local/bin/yt-dlp
RUN chmod a+rx /usr/local/bin/yt-dlp

WORKDIR /app
RUN wget https://download.pytorch.org/libtorch/cpu/libtorch-cxx11-abi-static-with-deps-2.0.0%2Bcpu.zip
RUN unzip libtorch-cxx11-abi-static-with-deps-2.0.0+cpu.zip

COPY . .
RUN LIBTORCH=/app/libtorch LD_LIBRARY_PATH=/app/libtorch/lib:$LD_LIBRARY_PATH cargo install --path .
CMD LIBTORCH=/app/libtorch LD_LIBRARY_PATH=/app/libtorch/lib:$LD_LIBRARY_PATH andrena
