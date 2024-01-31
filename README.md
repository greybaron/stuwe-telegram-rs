# Studentenwerk Leipzig Mensa Bot
A Telegram bot that crawls the Studentenwerk Leipzig Mensa site and returns todays' meals.

## Build Dependencies
* A working Rust toolchain
* SQLite3 development files (e.g. `libsqlite3-dev` on Debian)
* SSL development files (e.g. `libssl-dev` on Debian)

## Runtime Dependencies
* A Bot has to be created using [@BotFather](https://t.me/BotFather), which produces a Token
* If using the CampusDual feature, the `GEANT OV RSA CA 4` certificate must be installed. On most Linux distributions, this certificate is not shipped
* Any data that is persisted is saved to SQLite3 databases in the current working directory, so it should be ensured that the directory is writable and not volatile
