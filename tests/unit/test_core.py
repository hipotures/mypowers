import asyncio
from uuid import uuid4

import pytest
from conftest import admit_and_fresh, frame, request

from mypowers.contracts import AppError, Output


def test_independent_link_freshness_wall_clock_and_invalid(core):
    service, clock, _, session = core
    assert service.snapshot().controls.allowed
    revision = service.revision
    clock.epoch -= 10000
    assert service.snapshot().telemetry.state == "live"
    service.receive(frame(input_w=7), session)
    assert service.revision == revision
    service.receive(b"bad", session)
    assert service.snapshot().telemetry.state == "invalid"
    assert service.latest.sample.input_power_w == 7
    assert not service.snapshot().controls.allowed
    service.receive(frame(), session)
    service.receive(frame(), session)
    assert service.snapshot().controls.allowed
    clock.advance(3)
    service.tick()
    assert service.snapshot().telemetry.state == "stale"
    assert service.connection.link_connected
    assert service.revision > revision


def test_new_session_and_old_callback(core):
    service, _, writer, session = core
    sample = service.latest
    service.set_phase("station_not_found", retry_in_seconds=2, link_connected=False)
    assert service.connection.retry_in_seconds == 2
    new_session = service.begin_session(writer, "hci9")
    assert service.connection.retry_in_seconds is None
    service.receive(frame(31), session)
    assert service.latest == sample
    assert service.snapshot().telemetry.state == "stale"
    service.receive(frame(), new_session)
    assert not service.snapshot().controls.allowed
    service.receive(frame(), new_session)
    assert service.snapshot().controls.allowed
    assert service.connection.retry_in_seconds is None
    service.end_session()
    assert not service.snapshot().controls.allowed


@pytest.mark.parametrize("flags", [0, 8, 44, 76])
def test_unsupported_read_profile(core, flags):
    service, _, _, session = core
    service.receive(frame(flags), session)
    assert service.snapshot().telemetry.state == "live"
    assert not service.snapshot().controls.allowed


@pytest.mark.parametrize("output,mask", [(Output.AC, 2), (Output.DC, 1), (Output.LIGHT, 16)])
@pytest.mark.parametrize("source", [12, 13, 14, 15, 28, 29, 30, 31])
async def test_command_every_edge_two_confirmations(core, output, mask, source):
    service, clock, writer, session = core
    service.receive(frame(source), session)
    desired = not bool(source & mask)
    command = service.admit(output, request(service, desired), str(uuid4()))
    await asyncio.sleep(0)
    assert not writer.frames
    clock.advance(0.1)
    service.receive(frame(source), session)
    await asyncio.sleep(0.001)
    assert len(writer.frames) == 1
    clock.advance(0.1)
    service.receive(frame(source ^ mask, input_w=4), session)
    await asyncio.sleep(0.001)
    assert service.get_command(command.command_id).status == "sent"
    clock.advance(0.1)
    service.receive(frame(source ^ mask, output_w=8, minutes=1), session)
    await service.command_task
    result = service.get_command(command.command_id)
    assert result.status == "confirmed"
    assert len(result.confirmation_sequences) == 2
    assert result.source_flags == source and result.target_flags == source ^ mask
    assert not service.pending


async def test_no_change_requires_new_status_without_write(core):
    service, _, writer, _ = core
    command = await admit_and_fresh(core, enabled=False)
    await service.command_task
    assert service.get_command(command.command_id).status == "no_change"
    assert not writer.frames


async def test_busy_dedup_body_conflict_and_pause(core):
    service, _, _, _ = core
    body = request(service)
    admitted = service.admit(Output.AC, body, "same")
    assert service.admit(Output.AC, body, "same") == admitted
    with pytest.raises(AppError, match="different intention"):
        service.admit(Output.DC, body, "same")
    with pytest.raises(AppError, match="in progress"):
        service.admit(Output.DC, body, "new")
    with pytest.raises(AppError, match="Cannot pause"):
        service.connection_intent("paused")
    await service.close()
    assert service.get_command(admitted.command_id).status == "failed"


@pytest.mark.parametrize(
    "failure", ["invalid", "contradiction", "disconnect", "stale", "old_session"]
)
async def test_after_send_uncertainty_never_retries(core, failure):
    service, clock, writer, session = core
    command = await admit_and_fresh(core)
    await asyncio.sleep(0.001)
    clock.advance(0.1)
    service.receive(frame(14), session)
    await asyncio.sleep(0.001)
    if failure == "invalid":
        service.receive(b"bad", session)
    elif failure == "contradiction":
        service.receive(frame(12), session)
    elif failure == "disconnect":
        service.end_session()
    elif failure == "stale":
        clock.advance(3)
        service.tick()
    else:
        service.begin_session(writer, "hci8")
        service.receive(frame(14), session)
        service._notify(None)
    await service.command_task
    assert service.get_command(command.command_id).status == "unconfirmed"
    assert service.resync_required
    assert len(writer.frames) == 1
    with pytest.raises(AppError):
        service.admit(Output.DC, request(service), "another")


async def test_source_before_target_and_unrelated(core):
    service, clock, _, session = core
    command = await admit_and_fresh(core)
    await asyncio.sleep(0.001)
    for flags in (12, 12, 14, 14):
        clock.advance(0.1)
        service.receive(frame(flags), session)
        await asyncio.sleep(0.001)
    await service.command_task
    assert service.get_command(command.command_id).status == "confirmed"


async def test_revision_changes_while_waiting_rejected(core):
    service, clock, writer, session = core
    command = service.admit(Output.AC, request(service), "key")
    await asyncio.sleep(0)
    clock.advance(0.1)
    service.receive(frame(13), session)
    await service.command_task
    assert service.get_command(command.command_id).reason_code == "state_conflict"
    assert not writer.frames


async def test_lost_client_does_not_cancel_daemon_command_and_shutdown_uncertain(core):
    service, _, writer, _ = core
    command = await admit_and_fresh(core)
    await asyncio.sleep(0.001)
    assert len(writer.frames) == 1
    await service.close()
    assert service.get_command(command.command_id).status == "unconfirmed"
    assert service.resync_required


async def test_transport_exception_after_potential_send(core):
    service, _, writer, _ = core

    async def fail(_):
        raise TimeoutError

    writer.action = fail
    command = await admit_and_fresh(core)
    await service.command_task
    assert service.get_command(command.command_id).status == "unconfirmed"
    assert len(writer.frames) == 1


async def test_invalid_before_write_and_instance_revision_guards(core):
    service, _, writer, session = core
    with pytest.raises(AppError):
        service.admit(
            Output.AC, request(service).model_copy(update={"server_instance_id": uuid4()}), "x"
        )
    with pytest.raises(AppError):
        service.admit(
            Output.AC, request(service).model_copy(update={"expected_outputs_revision": 99}), "x"
        )
    command = service.admit(Output.AC, request(service), "key")
    await asyncio.sleep(0)
    service.receive(b"bad", session)
    await service.command_task
    assert service.get_command(command.command_id).status == "rejected"
    assert not writer.frames


def test_registry_unknown_expiry_and_stream_bounds(core):
    service, clock, _, _ = core
    with pytest.raises(AppError):
        service.get_command(str(uuid4()))
    queues = [service.subscribe() for _ in range(16)]
    with pytest.raises(AppError):
        service.subscribe()
    small = queues[0]
    for _ in range(130):
        service.publish()
    assert small.qsize() <= 128
    assert small.get_nowait()["type"] == "overflow"
    clock.advance(3601)
    service.prune()


async def test_command_notification_overflow_fails_closed(core):
    service, clock, writer, session = core

    async def flood(_):
        for _ in range(130):
            clock.advance(0.001)
            service.receive(frame(14), session)

    writer.action = flood
    command = await admit_and_fresh(core)
    await service.command_task
    assert service.get_command(command.command_id).status == "unconfirmed"
    assert service.notifications.qsize() <= 128
    assert len(writer.frames) == 1


async def test_admitted_but_not_sent_shutdown_and_eligibility_loss(core):
    service, _, writer, _ = core
    command = service.admit(Output.AC, request(service), "key")
    await asyncio.sleep(0)
    await service.close()
    assert service.get_command(command.command_id).status == "failed"
    assert not writer.frames
    assert service.snapshot().controls.reason_code == "shutting_down"
    with pytest.raises(AppError):
        service.connection_intent("running")


async def test_stale_queued_notification_and_lost_link_before_wait(core):
    service, clock, writer, session = core
    command = service.admit(Output.AC, request(service), "key")
    await asyncio.sleep(0)
    clock.advance(0.1)
    service.receive(frame(), session)
    clock.advance(3)
    await service.command_task
    assert service.get_command(command.command_id).reason_code == "stale_telemetry"
    assert not writer.frames


async def test_no_sample_deadline_prewrite_revocation_and_profile_policy(core):
    service, clock, writer, session = core
    with pytest.raises(AppError, match="deadline"):
        await service.next_observation(session, service.sequence, 0)
    service.receive(frame(8), session)
    assert service.eligible_reason() == "unqualified_profile"
    with pytest.raises(AppError) as caught:
        service.admit(Output.AC, request(service), "key")
    assert caught.value.status == 409
    service.address = "11:22:33:44:55:66"
    with pytest.raises(AppError) as caught:
        service.admit(Output.AC, request(service), "key")
    assert caught.value.status == 403
    assert not writer.frames


async def test_old_cutoff_ignored_and_no_writer_after_admission(core):
    from mypowers.core import Notification

    service, clock, writer, session = core
    command = service.admit(Output.AC, request(service), "key")
    await asyncio.sleep(0)
    service.notifications.put_nowait(
        Notification(service.latest, service.sequence, session, clock.mono)
    )
    clock.advance(0.1)
    service.receive(frame(), session)
    service.writer = None
    await service.command_task
    assert service.get_command(command.command_id).status == "rejected"
    assert not writer.frames


async def test_retained_registry_capacity_expiry_no_active_eviction(core):
    service, clock, writer, session = core
    for index in range(1001):
        command = service.admit(Output.AC, request(service, False), str(index))
        await asyncio.sleep(0)
        clock.advance(0.001)
        service.receive(frame(), session)
        await service.command_task
    assert len(service.commands) == 1000
    assert service.get_command(command.command_id).status == "no_change"
    clock.advance(3601)
    service.prune()
    assert not service.commands
    assert not writer.frames


def test_scan_rssi_age_is_monotonic_under_wall_clock_jump(core):
    service, clock, _, _ = core
    service.rssi = -67
    service.rssi_at = clock.time()
    service.rssi_monotonic = clock.monotonic()
    clock.advance(2)
    clock.epoch -= 3600
    assert service.snapshot().diagnostics["last_scan_rssi_age_seconds"] == 2


async def test_real_transport_deadline_may_have_sent_once(core):
    service, _, writer, _ = core

    async def hang(_):
        await asyncio.Event().wait()

    writer.action = hang
    command = await admit_and_fresh(core)
    await service.command_task
    assert service.get_command(command.command_id).status == "unconfirmed"
    assert len(writer.frames) == 1
    assert service.resync_required
