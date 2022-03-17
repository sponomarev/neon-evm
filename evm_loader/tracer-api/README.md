# Build
Building process is verified for Ubuntu 20.04

## Install requirements
    apt-get update
    apt-get install -y build-essential curl pkg-config libssl-dev libudev-dev clang git jq unzip

## Install Rust 
    curl https://sh.rustup.rs -sSf | sh -s -- -y --default-toolchain $(cat rust-toolchain)
    
also see `Dockerfile.build` to create a docker image with the Rust environment.

## Build the tracer-api
    export PATH="~/.cargo/bin:${PATH}"
    export NEON_REVISION="true"
    cargo build --release

Now you can find the built binary at `target/release/neon-tracer`

## Build a docker image (optionally)
    docker build --tag neon-tracer:latest -f Dockerfile.dist

# Prepare the environment

## Database

1. Install clickhouse-server version 21.8 or above.

2. Create a database and apply the sql commands from `clickhouse/202110061400_initial.up.sql`

3. Create two users:
    - the user with write permissions for the validator
    - the user with read permissions for the tracer-api (neon-tracer)


# Run

There are three methods to run neon-tracer.

## Run binary
    neon-tracer -l 0.0.0.0:8250 -c <clickhouse-server> -d <clickhouse-database> -u <clickhouse-user> -p <clickhouse-password>

## Run the Docker image
    docker run --rm -ti --network=host -p 0.0.0.0:8250:8250 neon-tracer:latest neon-tracer -l 0.0.0.0:8250 -c <clickhouse-server> -d <clickhouse-database> -u <clickhouse-user> -p <clickhouse-password>

## Run the Docker image with docker-compose
Create a neon-tracer-compose.yml file (see neon-tracer-compose.sample.yml) with content:

    version: "3.7"
    services:
      neon-tracer:
        network_mode: host
        image: neon-tracer:latest
        ports:
          - "0.0.0.0:8250:8250"
        command: neon-tracer -l 0.0.0.0:8250 -c <clickhouse-server> -d <clickhouse-database> -u <clickhouse-user> -p <clickhouse-password>

And run it with the command:

    docker-compose -f neon-tracer-compose.yml up -d


# Command-line arguments

`-l <bind-addrerss>:<port>` - Network interface address and port neon-tracer listens for connections.

`-c <clickhouse-server>` - Clickhouse server URL to connect to HTTP interface.
Example: http://clickhouse-server:8123/

`-d <clickhouse-database>` - Database name

`-u <clickhouse-user>` - User with read access

`-p <clickhouse-password>` - User password

