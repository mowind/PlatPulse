"""Aggregate-tier fixtures for the Node Detail Metric Window (issues #247, #248).

The longer Metric Windows are answered from the 1-minute and 5-minute buckets
the Server writes at ingestion (ADR 0012). The e2e harness provisions its
fixtures directly in SQLite, so this writes the buckets ingestion would have
written for Node A and its Agent's Host network: one observation every five
seconds, every bucket from `start` up to the bucket holding `end`, with the
newest bucket still open. `start-server.sh` seeds eight days once and its
refresher keeps the newest buckets advancing with the clock.

Issue #248 adds the block tiers (`node_block_aggregates`): Node A accepts one
block per observation instant, each with a known parent, so every bucket counts
as many intervals as blocks.
"""

import math
from datetime import datetime, timedelta, timezone

CADENCE = 5
GRAINS = (60, 300)
NODE_SERIES = (
    ("process_cpu_percent", 15.0, 6.0),
    ("process_memory_percent", 12.0, 1.5),
    ("data_directory_percent", 50.0, 0.4),
    ("peer_inbound_count", 2.0, 1.0),
    ("peer_outbound_count", 2.0, 1.0),
)
HOST_SERIES = (
    ("network_rx_bytes_per_sec", 820000.0, 120000.0),
    ("network_tx_bytes_per_sec", 210000.0, 40000.0),
)


def stamp(instant):
    return instant.strftime("%Y-%m-%dT%H:%M:%SZ")


def value_at(base, swing, seconds, whole):
    reading = base + swing * math.sin(seconds / 1800.0)
    return float(round(reading)) if whole else round(reading, 3)


def block_row(db, node_id, grain, bucket_start, instants, now_text):
    """The block bucket ingestion would have written for one block per instant."""
    transactions = [int(round(4 + 3 * math.sin(instant / 900.0))) for instant in instants]
    intervals = [5000 + int(round(800 * math.sin(instant / 1300.0))) for instant in instants]
    newest = datetime.fromtimestamp(instants[-1], timezone.utc)
    db.execute(
        "INSERT OR REPLACE INTO node_block_aggregates (node_id, grain_seconds, bucket_start, block_count, transaction_total, transaction_min, transaction_max, last_transaction_count, interval_count, interval_min_ms, interval_max_ms, last_interval_ms, last_interval_observed_at, first_observed_at, last_observed_at, last_received_at, max_gap_seconds, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        (
            node_id,
            grain,
            stamp(bucket_start),
            len(instants),
            sum(transactions),
            min(transactions),
            max(transactions),
            transactions[-1],
            len(instants),
            min(intervals),
            max(intervals),
            intervals[-1],
            stamp(newest),
            stamp(bucket_start),
            stamp(newest),
            stamp(newest),
            CADENCE if len(instants) > 1 else 0,
            now_text,
        ),
    )


def fill_buckets(db, node_id, agent_id, start, end, grains=GRAINS):
    """Upsert every bucket of the given tiers whose start lies in [start, end]."""
    end = end.replace(microsecond=0)
    now_text = stamp(end)
    for grain in grains:
        first = int(start.timestamp()) // grain * grain
        last = int(end.timestamp()) // grain * grain
        for bucket in range(first, last + 1, grain):
            bucket_start = datetime.fromtimestamp(bucket, timezone.utc)
            newest = min(bucket + grain - CADENCE, int(end.timestamp()))
            if newest < bucket:
                continue
            count = (newest - bucket) // CADENCE + 1
            instants = [bucket + step * CADENCE for step in range(count)]
            block_row(db, node_id, grain, bucket_start, instants, now_text)
            for table, scope_column, scope_key, series, dimension in (
                ("node_metric_aggregates", "node_id", node_id, NODE_SERIES, None),
                ("host_metric_aggregates", "agent_id", agent_id, HOST_SERIES, ""),
            ):
                for metric, base, swing in series:
                    whole = metric.startswith("peer_")
                    values = [value_at(base, swing, instant, whole) for instant in instants]
                    columns = [scope_column, "metric"]
                    params = [scope_key, metric]
                    if dimension is not None:
                        columns.append("dimension")
                        params.append(dimension)
                    columns += [
                        "grain_seconds",
                        "bucket_start",
                        "sample_count",
                        "min_value",
                        "max_value",
                        "last_value",
                        "first_observed_at",
                        "last_observed_at",
                        "last_received_at",
                        "max_gap_seconds",
                        "updated_at",
                    ]
                    params += [
                        grain,
                        stamp(bucket_start),
                        count,
                        min(values),
                        max(values),
                        values[-1],
                        stamp(bucket_start),
                        stamp(datetime.fromtimestamp(newest, timezone.utc)),
                        stamp(datetime.fromtimestamp(newest, timezone.utc)),
                        CADENCE if count > 1 else 0,
                        now_text,
                    ]
                    db.execute(
                        f"INSERT OR REPLACE INTO {table} ({', '.join(columns)}) VALUES ({', '.join('?' for _ in columns)})",
                        params,
                    )


def seed(db, node_id, agent_id, days=8):
    end = datetime.now(timezone.utc) - timedelta(seconds=5)
    # The 1-minute tier only answers the 6-hour window, so seven hours of it
    # are enough; the 5-minute tier covers the 7-day window and a day beyond.
    fill_buckets(db, node_id, agent_id, end - timedelta(hours=7), end, (60,))
    fill_buckets(db, node_id, agent_id, end - timedelta(days=days), end, (300,))
