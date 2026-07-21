"""Failure-mode ("chaos") tests for the event ingestion pipeline.

``test_ingest.py`` and ``test_system.py`` only cover the happy path: an RPC node
that answers on the first try with a single, complete event page, and a database
that never wobbles. Production is not that kind. This module exercises the four
failure modes that actually take ingestion down -- Soroban RPC timeouts,
truncated/paginated event pages, pruned ("ledger too old") ranges, and a
database connection dropped mid-transaction -- and asserts the pipeline neither
crashes unrecoverably nor corrupts already-committed data, and catches up on the
next poll cycle.

The harness (``FlakyRpcServer``, ``DroppingSessionFactory`` and the ``flaky_rpc``
/ ``dropping_db`` fixtures) lives in ``conftest.py`` so it is reusable by any
durability/idempotency fix that lands later.

Two modes are marked ``xfail`` on purpose: they encode behaviour a durable-cursor
fix is expected to add but which the current pipeline does not yet have. They are
the red half of a red/then/green check -- drop the ``xfail`` marker and they fail
today against real bugs (``fetch_events`` ignores the RPC pagination cursor;
``events_to_db`` has no idempotent-replay handling), and they flip to passing once
the fix lands.
"""

import pytest
from sqlalchemy import delete, func, select
from sqlalchemy.exc import IntegrityError, OperationalError
from stellar_sdk.exceptions import ConnectionError as SorobanConnectionError
from stellar_sdk.exceptions import SorobanRpcErrorResponse

from tansu.events.database import db_models
from tansu.events.database.session_factory import SessionFactory
from tansu.events.ingest import events_to_db, fetch_events
from tests.conftest import make_soroban_event

CONTRACT_ID = "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC"


def _rpc_events(project_key: str, n: int, *, start_ledger: int = 900):
    """`n` distinct Soroban RPC events for `project_key`, one per ledger."""
    return [
        make_soroban_event(
            ledger=start_ledger + i,
            action="commit",
            project_key=project_key,
            value=f"{project_key}-value-{i}",
        )
        for i in range(n)
    ]


def _db_events(project_key: str, values):
    return [
        db_models.Event(
            ledger=100 + i,
            action="commit",
            project_key=project_key,
            value=value,
        )
        for i, value in enumerate(values)
    ]


async def _count(project_key: str) -> int:
    async with SessionFactory() as session:
        result = await session.execute(
            select(func.count())
            .select_from(db_models.Event)
            .where(db_models.Event.project_key == project_key)
        )
        return result.scalar_one()


async def _purge(project_key: str) -> None:
    """Drop any rows for this key so the test is independent and re-runnable."""
    async with SessionFactory() as session:
        async with session.begin():
            await session.execute(
                delete(db_models.Event).where(
                    db_models.Event.project_key == project_key
                )
            )


@pytest.mark.parametrize(
    "failure_mode",
    [
        "timeout",
        "connection_drop",
        "pruned_ledger",
        "empty_page",
        "db_drop",
        pytest.param(
            "partial_page",
            marks=pytest.mark.xfail(
                reason=(
                    "fetch_events issues a single get_events() call and ignores the "
                    "response cursor, so a paginated/truncated page silently drops "
                    "every event beyond the first page. Needs the durable-cursor / "
                    "pagination fix."
                ),
                strict=False,
            ),
        ),
        pytest.param(
            "idempotent_replay",
            marks=pytest.mark.xfail(
                reason=(
                    "events_to_db does a plain INSERT with no ON CONFLICT handling, so "
                    "re-committing an already-persisted batch (as a retry after a "
                    "partial failure would) raises IntegrityError instead of being a "
                    "no-op. Needs the idempotency fix."
                ),
                strict=False,
            ),
        ),
    ],
)
async def test_ingest_survives_failure(failure_mode: str, flaky_rpc, dropping_db):
    project_key = f"chaos-{failure_mode}"
    await _purge(project_key)

    if failure_mode in ("timeout", "connection_drop"):
        # RPC blips on the first poll, then recovers. Ingestion must surface the
        # error (not hang or corrupt anything) and fully catch up on retry.
        server = flaky_rpc(
            events=_rpc_events(project_key, 3),
            transient_failures=1,
            failure=failure_mode,
        )

        with pytest.raises(SorobanConnectionError):
            fetch_events(contract_id=CONTRACT_ID, start_ledger=900)
        assert await _count(project_key) == 0  # nothing half-written

        events, latest_ledger = fetch_events(contract_id=CONTRACT_ID, start_ledger=900)
        await events_to_db(events)

        assert server.get_events_calls == 2  # failed once, retried once
        assert isinstance(latest_ledger, int)
        assert await _count(project_key) == 3

    elif failure_mode == "pruned_ledger":
        # The durable cursor points at history the node has pruned. The RPC
        # rejects it; ingestion recovers by clamping forward to oldest_ledger.
        server = flaky_rpc(
            events=_rpc_events(project_key, 3),
            oldest_ledger=800,
            latest_ledger=1000,
            failure="pruned_ledger",
        )

        with pytest.raises(SorobanRpcErrorResponse):
            fetch_events(contract_id=CONTRACT_ID, start_ledger=100)
        assert await _count(project_key) == 0

        events, _ = fetch_events(
            contract_id=CONTRACT_ID, start_ledger=server.oldest_ledger
        )
        await events_to_db(events)
        assert await _count(project_key) == 3

    elif failure_mode == "empty_page":
        # A poll cycle with no new events must be a clean no-op, not a crash.
        flaky_rpc(events=[])
        events, latest_ledger = fetch_events(contract_id=CONTRACT_ID, start_ledger=900)
        assert events == []
        assert isinstance(latest_ledger, int)
        await events_to_db(events)  # committing nothing is harmless
        assert await _count(project_key) == 0

    elif failure_mode == "db_drop":
        # Commit a first batch, then drop the connection mid-transaction on the
        # second. The drop must abort atomically -- the committed row survives,
        # the dropped batch leaves nothing behind -- and a retry catches up.
        await events_to_db(_db_events(project_key, ["committed"]))
        assert await _count(project_key) == 1

        with dropping_db():
            with pytest.raises(OperationalError):
                await events_to_db(_db_events(project_key, ["dropped"]))

        assert await _count(project_key) == 1  # no partial write from the drop

        await events_to_db(_db_events(project_key, ["dropped"]))  # next cycle
        assert await _count(project_key) == 2

    elif failure_mode == "partial_page":
        # RPC returns the 5 events across pages of 2, advertising more via the
        # cursor. A durable fetch must follow the cursor and return all 5.
        flaky_rpc(events=_rpc_events(project_key, 5), page_size=2)
        events, _ = fetch_events(contract_id=CONTRACT_ID, start_ledger=900)
        await events_to_db(events)
        assert await _count(project_key) == 5

    elif failure_mode == "idempotent_replay":
        # A retry after a partial failure re-delivers an already-committed batch.
        # Re-ingesting it must be a no-op, not a crash, and must not duplicate.
        batch = _db_events(project_key, ["a", "b", "c"])
        await events_to_db(batch)
        assert await _count(project_key) == 3

        replay = _db_events(project_key, ["a", "b", "c"])
        try:
            await events_to_db(replay)
        except IntegrityError:
            pytest.fail("re-committing an already-ingested batch was not idempotent")
        assert await _count(project_key) == 3

    else:  # pragma: no cover - guards against an unhandled new parameter
        raise AssertionError(f"unhandled failure_mode: {failure_mode}")

    await _purge(project_key)
