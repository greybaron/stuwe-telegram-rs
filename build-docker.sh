#!/usr/bin/env bash

# Function to build and push stuwe version
build_stuwe() {
  docker buildx build \
  --platform linux/amd64,linux/arm64 \
  --tag flschmidt/stuwe-telegram-rs --target stuwe-telegram-rs --push .
}

# Function to build and push mensi version
build_mensi() {
  docker buildx build \
  --platform linux/amd64,linux/arm64 \
  --tag flschmidt/mensi-telegram-rs --target mensi-telegram-rs --push .
}

# only needs to be run once
docker buildx create --name mybuilder --use

case $1 in
  stuwe)
    build_stuwe
    ;;
  mensi)
    build_mensi
    ;;
  all)
    build_stuwe
    build_mensi
    ;;
  *)
    echo "Error: Invalid argument. Use 'stuwe', 'mensi', or 'all'."
    exit 1
    ;;
esac
