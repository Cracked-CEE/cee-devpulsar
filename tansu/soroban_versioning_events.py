import asyncio

from tansu.events.ingest import run_poll_cycle

contract_id = "CCYM5OC6RTMEUHRK2BRU5YX4G4O745DPLPU4EXVTRCUN7JRJJWXEEXAB"
start_ledger = None

result = asyncio.run(run_poll_cycle(contract_id, start_ledger))
if result is None:
    print("Skipped: genesis backfill in progress")
else:
    events, _ = result
    print(events)
