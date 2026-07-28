"""Genesis-to-tip event backfill.

``fetch_events`` (see :mod:`tansu.events.ingest`) only ever looks ~20h back,
which is fine for the regular poll cycle but leaves no way to (re)ingest a
contract's entire on-chain history -- needed for disaster recovery, migrating
to a new DB, or onboarding an existing long-running deployment for the first
time.

``backfill_from_genesis`` paginates Soroban RPC's ``get_events`` from a given
genesis ledger up to the chain tip captured when the backfill starts,
persisting a checkpoint (last completed ledger + RPC page cursor) every
``checkpoint_interval`` ledgers so an interrupted run can resume without
re-processing everything, and without producing duplicate rows.

It coordinates with the regular poller (`tansu.events.ingest.run_poll_cycle`)
via a Postgres advisory lock (`tansu.events.lock`) keyed on the contract id,
so the two never process overlapping ledger ranges concurrently.
"""

import argparse
import asyncio
import datetime

import stellar_sdk
from sqlalchemy.dialects.postgresql import insert as pg_insert
from sqlalchemy.ext.asyncio import async_sessionmaker
from stellar_sdk.exceptions import ConnectionError as SorobanConnectionError
from stellar_sdk.exceptions import SorobanRpcErrorResponse

from tansu.events import ingest, lock
from tansu.events.database import db_models
from tansu.events.database.session_factory import SessionFactory, engine
from tansu.events.log import logger

# Soroban RPC event pages are capped server-side; keep well under it.
DEFAULT_PAGE_LIMIT = 1000
# Minimum pause between successful page requests, to stay under RPC rate limits.
DEFAULT_PAGE_DELAY_SECONDS = 0.1
# Retry/backoff for transient RPC failures (timeouts, connection drops).
DEFAULT_MAX_RETRIES = 5
DEFAULT_BACKOFF_BASE_SECONDS = 1.0
DEFAULT_BACKOFF_MAX_SECONDS = 30.0


async def _fetch_page_with_backoff(
    soroban_server: stellar_sdk.SorobanServer,
    contract_id: str,
    *,
    start_ledger: int | None,
    end_ledger: int,
    cursor: str | None,
    limit: int,
    max_retries: int,
    backoff_base: float,
    backoff_max: float,
):
    """`get_events` for one page, retrying transient RPC failures with backoff."""
    kwargs = dict(filters=[ingest.event_filter(contract_id)], limit=limit)
    if cursor:
        kwargs["cursor"] = cursor
    else:
        kwargs["start_ledger"] = start_ledger
        kwargs["end_ledger"] = end_ledger

    attempt = 0
    while True:
        try:
            return await asyncio.to_thread(soroban_server.get_events, **kwargs)
        except (SorobanConnectionError, SorobanRpcErrorResponse):
            attempt += 1
            if attempt > max_retries:
                raise
            delay = min(backoff_base * (2 ** (attempt - 1)), backoff_max)
            logger.warning(
                f"Soroban RPC error fetching events for {contract_id} "
                f"(attempt {attempt}/{max_retries}); retrying in {delay:.1f}s"
            )
            await asyncio.sleep(delay)


async def _load_checkpoint(
    session_factory: async_sessionmaker, contract_id: str
) -> db_models.BackfillCheckpoint | None:
    async with session_factory() as session:
        return await session.get(db_models.BackfillCheckpoint, contract_id)


async def _commit_progress(
    session_factory: async_sessionmaker,
    contract_id: str,
    events: list[db_models.Event],
    *,
    genesis_ledger: int,
    tip_ledger: int,
    last_completed_ledger: int,
    cursor: str | None,
    completed: bool,
) -> None:
    """Atomically write a page batch and advance the checkpoint.

    Events are inserted with `ON CONFLICT DO NOTHING` (keyed on the same
    `(action, project_key, value)` unique constraint the regular poller
    relies on) so replaying a page already committed before an interruption
    -- which can happen since the checkpoint only advances every
    `checkpoint_interval` ledgers, not every page -- is a no-op rather than
    an integrity error.
    """
    async with session_factory() as session:
        async with session.begin():
            if events:
                rows = [
                    {
                        "ledger": e.ledger,
                        "action": e.action,
                        "project_key": e.project_key,
                        "value": e.value,
                    }
                    for e in events
                ]
                stmt = pg_insert(db_models.Event).values(rows)
                stmt = stmt.on_conflict_do_nothing(
                    constraint="_action_project_key_value"
                )
                await session.execute(stmt)

            checkpoint_stmt = pg_insert(db_models.BackfillCheckpoint).values(
                contract_id=contract_id,
                genesis_ledger=genesis_ledger,
                tip_ledger=tip_ledger,
                last_completed_ledger=last_completed_ledger,
                cursor=cursor,
                completed_at=datetime.datetime.now(datetime.timezone.utc)
                if completed
                else None,
            )
            checkpoint_stmt = checkpoint_stmt.on_conflict_do_update(
                index_elements=[db_models.BackfillCheckpoint.contract_id],
                set_=dict(
                    last_completed_ledger=checkpoint_stmt.excluded.last_completed_ledger,
                    cursor=checkpoint_stmt.excluded.cursor,
                    completed_at=checkpoint_stmt.excluded.completed_at,
                ),
            )
            await session.execute(checkpoint_stmt)


async def backfill_from_genesis(
    contract_id: str,
    genesis_ledger: int,
    session_factory: async_sessionmaker = SessionFactory,
    checkpoint_interval: int = 1000,
    *,
    page_limit: int = DEFAULT_PAGE_LIMIT,
    page_delay_seconds: float = DEFAULT_PAGE_DELAY_SECONDS,
    max_retries: int = DEFAULT_MAX_RETRIES,
    backoff_base: float = DEFAULT_BACKOFF_BASE_SECONDS,
    backoff_max: float = DEFAULT_BACKOFF_MAX_SECONDS,
) -> db_models.BackfillCheckpoint:
    """Ingest every event for `contract_id` from `genesis_ledger` to the chain tip.

    Resumable: progress is checkpointed (last completed ledger + RPC page
    cursor) every `checkpoint_interval` ledgers, so interrupting and
    re-invoking this with the same `contract_id`/`genesis_ledger` picks up
    where it left off. The chain tip is captured once, on the *first* run,
    and reused on every resume, so an interrupted-then-resumed backfill
    covers exactly the same ledger range -- and therefore produces the same
    final event set -- as a single uninterrupted run. Calling this again
    after completion is a no-op.

    Refuses to run while the regular poll cycle (`ingest.run_poll_cycle`)
    holds the contract's advisory lock, and vice versa, so the two never
    process overlapping ledger ranges concurrently.
    """
    checkpoint = await _load_checkpoint(session_factory, contract_id)
    if checkpoint is not None and checkpoint.genesis_ledger != genesis_ledger:
        raise ValueError(
            f"existing checkpoint for {contract_id!r} was started from genesis "
            f"ledger {checkpoint.genesis_ledger}, not {genesis_ledger}; "
            "backfilling from a different genesis would produce an inconsistent "
            "final event set"
        )

    if checkpoint is not None and checkpoint.completed_at is not None:
        logger.info(
            f"Backfill for {contract_id} already completed at "
            f"ledger {checkpoint.last_completed_ledger}; nothing to do"
        )
        return checkpoint

    soroban_server = stellar_sdk.SorobanServer()

    async with engine.connect() as conn:
        async with lock.contract_lock(conn, contract_id):
            if checkpoint is None:
                tip_ledger = soroban_server.get_latest_ledger().sequence
                start_ledger = genesis_ledger
                resume_cursor = None
                last_checkpointed_ledger = genesis_ledger - 1
            else:
                tip_ledger = checkpoint.tip_ledger
                start_ledger = checkpoint.last_completed_ledger + 1
                resume_cursor = checkpoint.cursor
                last_checkpointed_ledger = checkpoint.last_completed_ledger

            logger.info(
                f"Backfilling {contract_id} from ledger {start_ledger} to tip "
                f"{tip_ledger} (checkpoint every {checkpoint_interval} ledgers)"
            )

            pending_events: list[db_models.Event] = []
            max_ledger_seen = last_checkpointed_ledger
            cursor = resume_cursor

            while True:
                page = await _fetch_page_with_backoff(
                    soroban_server,
                    contract_id,
                    start_ledger=start_ledger if not cursor else None,
                    end_ledger=tip_ledger,
                    cursor=cursor,
                    limit=page_limit,
                    max_retries=max_retries,
                    backoff_base=backoff_base,
                    backoff_max=backoff_max,
                )

                mapped = ingest.map_events(page.events)
                pending_events.extend(mapped)
                if mapped:
                    max_ledger_seen = max(
                        max_ledger_seen, max(e.ledger for e in mapped)
                    )

                cursor = page.cursor or None
                reached_tip = cursor is None

                should_checkpoint = reached_tip or (
                    max_ledger_seen - last_checkpointed_ledger >= checkpoint_interval
                )

                if should_checkpoint:
                    completed_ledger = tip_ledger if reached_tip else max_ledger_seen
                    await _commit_progress(
                        session_factory,
                        contract_id,
                        pending_events,
                        genesis_ledger=genesis_ledger,
                        tip_ledger=tip_ledger,
                        last_completed_ledger=completed_ledger,
                        cursor=cursor,
                        completed=reached_tip,
                    )
                    logger.info(
                        f"Backfill checkpoint for {contract_id}: "
                        f"{completed_ledger}/{tip_ledger}"
                        f" ({len(pending_events)} events this batch)"
                    )
                    pending_events = []
                    last_checkpointed_ledger = completed_ledger

                if reached_tip:
                    break

                await asyncio.sleep(page_delay_seconds)

    return await _load_checkpoint(session_factory, contract_id)


def _parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Backfill a contract's on-chain events from genesis to tip."
    )
    parser.add_argument("--contract-id", required=True)
    parser.add_argument(
        "--genesis-ledger",
        type=int,
        required=True,
        help="First ledger to scan (the contract's deployment ledger, or earlier).",
    )
    parser.add_argument(
        "--checkpoint-interval",
        type=int,
        default=1000,
        help="Persist progress every N ledgers (default: 1000).",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> None:
    args = _parse_args(argv)
    checkpoint = asyncio.run(
        backfill_from_genesis(
            contract_id=args.contract_id,
            genesis_ledger=args.genesis_ledger,
            checkpoint_interval=args.checkpoint_interval,
        )
    )
    logger.info(
        f"Backfill for {args.contract_id} finished at ledger "
        f"{checkpoint.last_completed_ledger} (tip {checkpoint.tip_ledger})"
    )


if __name__ == "__main__":
    main()
