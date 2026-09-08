# shaipot - Shaicoin RandomX miner

Shaicoin replaced its ShaiHive (Hamiltonian cycle) proof of work with a
vendored RandomX fork on 2026-09-01. This is that miner.

## Why not xmrig

Shaicoin's RandomX changes the Argon2 salt to `ShaicoinRandomX-v2\x01` and
rebalances six instruction frequencies, specifically so Monero hashpower cannot
be repointed at it. A stock xmrig or any crates.io randomx runs at full speed
and produces hashes this chain has never seen - with no error anywhere. This
miner links the exact RandomX vendored in the node tree, and a unit test pins
the known-answer vector so a wrong build fails `cargo test` instead of silently
mining nothing.

## Build

    cargo build --release

Needs cmake and a C++ compiler; RandomX is built from `randomx/`.

## Run

    ./target/release/shaipot \
      --address sh1q... \
      --pool wss://mine.shaicoin-mining.com/ \
      --threads 4

Options:
  --threads N   worker threads (default: all cores)
  --light       ~256 MB instead of a 2.3 GB dataset, roughly 10x slower

## Huge pages

Fast mode allocates a 2.3 GB dataset. Huge pages are worth 20-30%:

    sudo sysctl -w vm.nr_hugepages=1280

The miner prints whether it got them. Without them it still works, just slower.

## Protocol

    pool -> miner  {"type":"job","job_id":..,"data":<224 hex>,"seed":<120 hex>,"target":<64 hex>}
    miner -> pool  {"type":"submit","miner_id":"sh1q..","job_id":..,"nonce":<8 hex>}

`data` is the serialized 112-byte header. The nonce is spliced at **byte 76**,
not appended - the 32-byte extension commitment follows it, unlike Bitcoin's
80-byte header where the nonce is last. `nonce` is those four bytes in header
(little-endian) order, sent verbatim.

Proof of work is `RandomX(seed, SHA256d(header)) <= target`, with the RandomX
output read little-endian, matching the node's uint256 handling.
