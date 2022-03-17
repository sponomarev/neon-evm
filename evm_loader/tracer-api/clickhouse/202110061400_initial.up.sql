CREATE TABLE accounts(
    date_time DateTime64(9, 'UTC'),
    transaction_signature FixedString(64),
    public_key FixedString(32),
    lamports UInt64,
    data String,
    owner FixedString(32),
    executable UInt8,
    rent_epoch UInt64,
    INDEX transaction_signature_idx transaction_signature TYPE bloom_filter GRANULARITY 3,
    INDEX public_key_idx public_key TYPE bloom_filter GRANULARITY 3,
    INDEX date_time_idx date_time TYPE minmax GRANULARITY 1
)
ENGINE = MergeTree()
ORDER BY date_time
PARTITION BY (toYYYYMM(date_time), substring(public_key, 1, 1));

CREATE TABLE accounts_after_transaction(
    date_time DateTime64(9, 'UTC'),
    transaction_signature FixedString(64),
    public_key FixedString(32),
    lamports UInt64,
    data String,
    owner FixedString(32),
    executable UInt8,
    rent_epoch UInt64,
    INDEX transaction_signature_idx transaction_signature TYPE bloom_filter GRANULARITY 3,
    INDEX public_key_idx public_key TYPE bloom_filter GRANULARITY 3,
    INDEX date_time_idx date_time TYPE minmax GRANULARITY 1
)
ENGINE = MergeTree()
ORDER BY date_time
PARTITION BY (toYYYYMM(date_time), substring(public_key, 1, 1));

CREATE TABLE transactions(
    date_time DateTime64(9, 'UTC'),
    transaction_signature FixedString(64),
    slot UInt64,
    message String,
    logs Array(String),
    INDEX transaction_signature_idx transaction_signature TYPE bloom_filter GRANULARITY 3,
    INDEX slot_idx slot TYPE bloom_filter GRANULARITY 3,
    INDEX date_time_idx date_time TYPE minmax GRANULARITY 1
)
ENGINE = MergeTree()
ORDER BY date_time
PARTITION BY toYYYYMM(date_time);

CREATE TABLE evm_transactions(
    date_time DateTime64(9, 'UTC'),
    transaction_signature FixedString(64),
    eth_transaction_signature FixedString(64),
    eth_from_addr FixedString(20),
    eth_to_addr Nullable(FixedString(20)),
    INDEX eth_transaction_signature_idx eth_transaction_signature TYPE bloom_filter GRANULARITY 3,
    INDEX eth_from_addr_idx eth_from_addr TYPE bloom_filter GRANULARITY 3,
    INDEX eth_to_addr_idx eth_to_addr TYPE bloom_filter GRANULARITY 3,
    INDEX date_time_idx date_time TYPE minmax GRANULARITY 1
)
ENGINE = MergeTree()
ORDER BY date_time
PARTITION BY toYYYYMM(date_time);

CREATE TABLE pruned_transactions(
    date_time DateTime64(9, 'UTC'),
    slot UInt64,
    INDEX slot_idx slot TYPE minmax GRANULARITY 1
)
ENGINE = MergeTree()
ORDER BY date_time
PARTITION BY toYYYYMM(date_time);
