"""Backfill checkpoint

Revision ID: a3f1c9d4e2b7
Revises: fcd0be8495da
Create Date: 2026-07-28 00:00:00.000000+00:00

"""

from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa


# revision identifiers, used by Alembic.
revision: str = "a3f1c9d4e2b7"
down_revision: Union[str, None] = "fcd0be8495da"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.create_table(
        "backfill_checkpoint",
        sa.Column("contract_id", sa.String(), nullable=False),
        sa.Column("genesis_ledger", sa.Integer(), nullable=False),
        sa.Column("tip_ledger", sa.Integer(), nullable=False),
        sa.Column("last_completed_ledger", sa.Integer(), nullable=False),
        sa.Column("cursor", sa.String(), nullable=True),
        sa.Column("completed_at", sa.DateTime(timezone=True), nullable=True),
        sa.Column(
            "updated_at",
            sa.DateTime(timezone=True),
            server_default=sa.func.now(),
            nullable=False,
        ),
        sa.PrimaryKeyConstraint("contract_id"),
    )


def downgrade() -> None:
    op.drop_table("backfill_checkpoint")
