import stellar_sdk
import stellar_sdk.xdr
from stellar_sdk.soroban_rpc import EventFilter, EventFilterType

from tansu.events.database import db_models, SessionFactory
from tansu.events.database.session_factory import engine
from tansu.events import api_models, lock
from tansu.events.log import logger


def event_filter(contract_id: str) -> EventFilter:
    return EventFilter(
        event_type=EventFilterType.CONTRACT,
        contract_ids=[contract_id],
        # can filter by projects or event type using topics
        # topics=[["*", "*"], ],
    )


def map_events(raw_events) -> list[db_models.Event]:
    """Map raw Soroban RPC event entries to `db_models.Event` rows."""
    events_ = []
    for event in raw_events:
        event = api_models.Event(
            ledger=event.ledger,
            action=event.topic[0],
            project_key=event.topic[1],
            value=event.value,
        )
        events_.append(db_models.Event(**event.model_dump()))
    return events_


def fetch_events(
    contract_id: str, start_ledger: int | None = None
) -> tuple[list[db_models.Event], int]:
    """Fetch events-events from Soroban RPC."""
    # stellar_sdk.soroban_server_async.SorobanServerAsync
    soroban_server = stellar_sdk.SorobanServer()
    if start_ledger is None:
        last_ledger = soroban_server.get_latest_ledger().sequence
        start_ledger = int(last_ledger - (3600 / 6 * 20))  # around 20h back

    res = soroban_server.get_events(
        start_ledger,
        filters=[event_filter(contract_id)],
    )

    latest_ledger = res.latest_ledger
    events_ = map_events(res.events)

    return events_, latest_ledger


async def events_to_db(events: list[db_models.Event]) -> None:
    """Commit events-events to DB."""
    async with SessionFactory() as session:
        async with session.begin():
            session.add_all(events)


async def run_poll_cycle(
    contract_id: str, start_ledger: int | None = None
) -> tuple[list[db_models.Event], int] | None:
    """Run one fetch_events + events_to_db cycle for `contract_id`.

    Skips the cycle (returns None) instead of running if a genesis backfill
    (`tansu.events.backfill.backfill_from_genesis`) currently holds the
    contract's advisory lock, so the poller never writes into a ledger range
    the backfill is still working through.
    """
    async with engine.connect() as conn:
        try:
            async with lock.contract_lock(conn, contract_id):
                events, latest_ledger = fetch_events(contract_id, start_ledger)
                await events_to_db(events)
                return events, latest_ledger
        except lock.ContractLockedError:
            logger.info(
                f"Skipping poll cycle for {contract_id}: genesis backfill in progress"
            )
            return None
