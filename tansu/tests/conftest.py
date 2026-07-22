from contextlib import contextmanager

import pytest
import stellar_sdk.scval
from httpx import AsyncClient, ASGITransport
from sqlalchemy import create_engine
from sqlalchemy.exc import OperationalError
from stellar_sdk.exceptions import ConnectionError as SorobanConnectionError
from stellar_sdk.exceptions import SorobanRpcErrorResponse
from stellar_sdk.soroban_rpc import (
    EventFilterType,
    EventInfo,
    GetEventsResponse,
    GetLatestLedgerResponse,
)

from tansu.events import ingest
from tansu.events.app import create_app
from tansu.events.database import db_models
from tansu.events.database.session_factory import (
    conn_str,
    SqlAlchemyBase,
    SessionFactory,
)
from tansu.events.ingest import events_to_db


TANSU_PROJECT_KEY = "37ae83c06fde1043724743335ac2f3919307892ee6307cce8c0c63eaa549e156"


# ----- API -----


@pytest.fixture(scope="session", autouse=True)
def app():
    app = create_app()
    yield app


@pytest.fixture(scope="session")
async def a_client(app):
    async with AsyncClient(
        transport=ASGITransport(app=app), base_url="http://test"
    ) as c:
        yield c


# ----- DATABASE -----

DEBUG = False
DROP_AFTER_TESTS = False


@pytest.fixture(scope="session")
def engine():
    engine_ = create_engine(conn_str, echo=DEBUG)
    yield engine_
    engine_.dispose()


@pytest.fixture(scope="session", autouse=True)
def tables(engine):
    print("\nCleaning tables")
    SqlAlchemyBase.metadata.drop_all(engine)
    print("\nCreating tables")
    SqlAlchemyBase.metadata.create_all(engine)
    yield
    if DROP_AFTER_TESTS:
        print("\nCleaning tables")
        SqlAlchemyBase.metadata.drop_all(engine)


@pytest.fixture(scope="session", autouse=True)
async def fake_events(tables):
    event_1 = db_models.Event(
        ledger=4,
        action="commit",
        project_key=TANSU_PROJECT_KEY,
        value="04cea5e23c7c50ae3dc304218314f21e7164c9d2",
    )
    event_2 = db_models.Event(
        ledger=1,
        action="register",
        project_key=TANSU_PROJECT_KEY,
        value="bed45eef20315c731cc5912c7e2aa626b7cf2a45",
    )

    await events_to_db([event_1, event_2])

    async with SessionFactory() as session:
        async with session.begin():
            session.add(db_models.LatestLedger(ledger=4))


# ----- CHAOS / FAILURE-MODE HARNESS -----
#
# A reusable test double for stellar_sdk.SorobanServer plus a failing DB
# session factory. Together they let tests drive fetch_events()/events_to_db()
# through the failure modes that actually happen against a Soroban RPC node in
# production -- RPC timeouts, truncated/paginated event pages, pruned ("ledger
# too old") ranges and connection drops mid-transaction -- without any network
# or a live RPC. See test_chaos_ingest.py for the assertions built on top.


def make_soroban_event(
    ledger: int,
    action: str,
    project_key: str,
    value: str,
    contract_id: str = "C" * 56,
) -> EventInfo:
    """Build an ``EventInfo`` shaped exactly like a real Soroban RPC event.

    ``topic`` and ``value`` are XDR-encoded (as the RPC returns them) so they
    round-trip through ``api_models.Event``'s ``parse_xdr`` validator the same
    way production data does.
    """
    return EventInfo(
        type=EventFilterType.CONTRACT.value,
        ledger=ledger,
        ledgerClosedAt="2024-01-01T00:00:00Z",
        contractId=contract_id,
        id=f"{ledger}-{value}",
        topic=[
            stellar_sdk.scval.to_symbol(action).to_xdr(),
            stellar_sdk.scval.to_string(project_key).to_xdr(),
        ],
        value=stellar_sdk.scval.to_string(value).to_xdr(),
        inSuccessfulContractCall=True,
        operationIndex=0,
        transactionIndex=0,
        txHash="00" * 32,
    )


class FlakyRpcServer:
    """Configurable stand-in for ``stellar_sdk.SorobanServer``.

    Only the two methods ``fetch_events`` touches are implemented:
    ``get_latest_ledger`` and ``get_events``. Behaviour is driven by the
    constructor so a single double can reproduce every failure mode:

    * ``transient_failures`` + ``failure="timeout"``/``"connection_drop"`` --
      raise a network error on the first N ``get_events`` calls, then succeed
      (models an RPC that recovers on retry / the next poll cycle).
    * ``failure="pruned_ledger"`` -- raise ``SorobanRpcErrorResponse`` whenever
      the requested ``start_ledger`` is older than ``oldest_ledger`` (the
      "ledger too old" / pruned-history error).
    * ``page_size`` -- return events one truncated page at a time, exposing the
      next page through the response ``cursor`` (models RPC pagination).

    Every call is counted so tests can assert on retry/pagination behaviour.
    """

    def __init__(
        self,
        events: list[EventInfo] | None = None,
        *,
        latest_ledger: int = 1000,
        oldest_ledger: int = 800,
        page_size: int | None = None,
        transient_failures: int = 0,
        failure: str | None = None,
    ) -> None:
        self._events = list(events or [])
        self.latest_ledger = latest_ledger
        self.oldest_ledger = oldest_ledger
        self.page_size = page_size
        self.transient_failures = transient_failures
        self.failure = failure
        self.get_events_calls = 0
        self.get_latest_ledger_calls = 0

    def get_latest_ledger(self) -> GetLatestLedgerResponse:
        self.get_latest_ledger_calls += 1
        return GetLatestLedgerResponse(
            id="latest",
            protocolVersion=22,
            sequence=self.latest_ledger,
            closeTime=1700000000,
            headerXdr="",
            metadataXdr="",
        )

    def get_events(
        self,
        start_ledger=None,
        end_ledger=None,
        filters=None,
        cursor=None,
        limit=None,
    ) -> GetEventsResponse:
        self.get_events_calls += 1

        # Transient network failure that clears after `transient_failures` calls.
        if self.get_events_calls <= self.transient_failures:
            if self.failure == "timeout":
                raise SorobanConnectionError("HTTPSConnectionPool: Read timed out.")
            if self.failure == "connection_drop":
                raise SorobanConnectionError(
                    "('Connection aborted.', ConnectionResetError(104, ...))"
                )

        # Requested history has been pruned from the RPC node.
        if (
            self.failure == "pruned_ledger"
            and start_ledger is not None
            and start_ledger < self.oldest_ledger
        ):
            raise SorobanRpcErrorResponse(
                -32600,
                f"startLedger must be within the ledger range: "
                f"{self.oldest_ledger} - {self.latest_ledger}",
            )

        if self.page_size:
            offset = int(cursor) if cursor else 0
            page = self._events[offset : offset + self.page_size]
            next_offset = offset + self.page_size
            has_more = next_offset < len(self._events)
            return self._response(page, str(next_offset) if has_more else "")

        return self._response(list(self._events), "")

    def _response(self, events: list[EventInfo], cursor: str) -> GetEventsResponse:
        return GetEventsResponse(
            events=events,
            latestLedger=self.latest_ledger,
            oldestLedger=self.oldest_ledger,
            latestLedgerCloseTime=1700000000,
            oldestLedgerCloseTime=1699000000,
            cursor=cursor,
        )


class DroppingSessionFactory:
    """DB session factory that drops the connection mid-transaction.

    Mimics enough of ``SessionFactory`` for ``events_to_db`` -- an async context
    manager whose ``session.begin()`` block raises ``OperationalError`` at commit
    time, exactly as a severed Postgres connection would. Because it fires inside
    the single transaction, nothing is committed, so it also proves the pipeline
    cannot leave partially-written data behind.
    """

    class _Session:
        def add_all(self, events):
            self._events = events

        async def __aenter__(self):
            return self

        async def __aexit__(self, *exc):
            return False

        def begin(self):
            class _Transaction:
                async def __aenter__(self_inner):
                    return None

                async def __aexit__(self_inner, exc_type, exc, tb):
                    raise OperationalError(
                        "INSERT INTO event ...",
                        {},
                        Exception("server closed the connection unexpectedly"),
                    )

            return _Transaction()

    def __call__(self):
        return self._Session()


@pytest.fixture
def flaky_rpc(monkeypatch):
    """Install a :class:`FlakyRpcServer` in place of ``SorobanServer``.

    Returns a factory: call it with :class:`FlakyRpcServer` keyword arguments to
    configure the failure behaviour for the test. ``fetch_events`` will then talk
    to the returned double instead of a live RPC node.
    """

    def _install(**config) -> FlakyRpcServer:
        server = FlakyRpcServer(**config)
        monkeypatch.setattr(ingest.stellar_sdk, "SorobanServer", lambda *a, **k: server)
        return server

    return _install


@pytest.fixture
def dropping_db():
    """Make ``events_to_db`` drop its connection mid-transaction on demand.

    Returns a context manager: inside the ``with`` block ``ingest.SessionFactory``
    is a :class:`DroppingSessionFactory`; on exit the real factory is restored, so
    the same test can then assert that a retry against the live DB catches up.
    """

    @contextmanager
    def _drop():
        original = ingest.SessionFactory
        ingest.SessionFactory = DroppingSessionFactory()
        try:
            yield
        finally:
            ingest.SessionFactory = original

    return _drop
