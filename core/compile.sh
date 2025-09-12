#!/bin/sh

base=$(dirname "$0")

function compile {
    for file in $1/*; do
        ../slang/build/Release/bin/slangc $file -fvk-use-scalar-layout -warnings-disable 39001 -I$base/shaders/include -profile spirv_1_6 -target spirv -o $base/shaders/bin/$(basename $file).spv -fvk-use-entrypoint-name
    done
}

compile $base/shaders/passes
