# download if libtorch is not present
if [ ! -d "libtorch" ]; then
    wget https://download.pytorch.org/libtorch/cpu/libtorch-cxx11-abi-static-with-deps-2.0.1%2Bcpu.zip
    unzip libtorch-cxx11-abi-static-with-deps-2.0.1+cpu.zip
    rm libtorch-cxx11-abi-static-with-deps-2.0.1+cpu.zip
fi

echo "Environment variables set"
export LIBTORCH=$PWD/libtorch
export LD_LIBRARY_PATH=$LIBTORCH/lib:$LD_LIBRARY_PATH
