#!/bin/bash

set -o errexit
set -o nounset
set -o pipefail
set -o xtrace

readonly PROJECT_NAME=rwled
readonly TARGET_HOST=pi@raspberrypi
readonly TARGET_PATH=/home/pi/$PROJECT_NAME.dev
readonly TARGET_ARCH=arm-unknown-linux-musleabihf
readonly SOURCE_PATH=./target/${TARGET_ARCH}/release/$PROJECT_NAME

cross build --release --target=${TARGET_ARCH}
rsync ${SOURCE_PATH} ${TARGET_HOST}:${TARGET_PATH}
ssh -t ${TARGET_HOST} env RUST_BACKTRACE=1 ${TARGET_PATH}
