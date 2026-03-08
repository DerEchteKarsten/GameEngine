#!/usr/bin/env bash

base=$(dirname "$0")

function compile {
    for file in $1/*.slang; do
        #-O3 -g0
        $base/../slang/build/Release/bin/slangc $file -g3 -warnings-disable 39001 -I$base/include -profile spirv_1_6 -target spirv -o $base/bin/$(basename $file).spv -fvk-use-entrypoint-name -reflection-json $base/bin/$(basename $file).json 
    done
}

compile $base/passes