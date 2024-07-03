#!/usr/bin/env bash
# replace the --tag appropriately
# use --load instead of --push to use the image locally

# only needs to be ran once
docker buildx create --name mybuilder --use

# build and push stuwe version
docker buildx build \
--platform linux/amd64,linux/arm64 \
--tag flschmidt/stuwe-telegram-rs --target stuwe-telegram-rs --push .

# build and push mensi version (reuses builder)
docker buildx build \
--platform linux/amd64,linux/arm64 \
--tag flschmidt/mensi-telegram-rs --target mensi-telegram-rs --push .

