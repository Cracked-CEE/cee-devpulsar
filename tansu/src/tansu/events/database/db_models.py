import datetime

import sqlalchemy
from sqlalchemy import orm, UniqueConstraint

from tansu.events.database.session_factory import SqlAlchemyBase


class Event(SqlAlchemyBase):
    __tablename__ = "event"

    id: orm.Mapped[int] = orm.mapped_column(primary_key=True)
    ledger: orm.Mapped[int]
    action: orm.Mapped[str] = orm.mapped_column(index=True)
    project_key: orm.Mapped[str] = orm.mapped_column(index=True)
    value: orm.Mapped[str]

    __table_args__ = (
        UniqueConstraint(
            "action", "project_key", "value", name="_action_project_key_value"
        ),
    )


class LatestLedger(SqlAlchemyBase):
    __tablename__ = "latest_ledger"

    ledger: orm.Mapped[int] = orm.mapped_column(primary_key=True)


class BackfillCheckpoint(SqlAlchemyBase):
    """Progress marker for a genesis-to-tip backfill (one row per contract).

    ``genesis_ledger``/``tip_ledger`` are pinned on the first run and reused on
    every resume so an interrupted-then-resumed backfill covers exactly the
    same ledger range as an uninterrupted one. ``cursor`` lets a resume pick up
    mid-page; ``last_completed_ledger`` is the ledger-granularity fallback used
    when the RPC's cursor can no longer be trusted (e.g. reused against a
    different node).
    """

    __tablename__ = "backfill_checkpoint"

    contract_id: orm.Mapped[str] = orm.mapped_column(primary_key=True)
    genesis_ledger: orm.Mapped[int]
    tip_ledger: orm.Mapped[int]
    last_completed_ledger: orm.Mapped[int]
    cursor: orm.Mapped[str | None]
    completed_at: orm.Mapped[datetime.datetime | None]
    updated_at: orm.Mapped[datetime.datetime] = orm.mapped_column(
        server_default=sqlalchemy.func.now(),
        onupdate=sqlalchemy.func.now(),
    )
