# API and device contracts

## Identity and permissions

GET /api/auth/me returns auth_id (Supabase subject), account_id, profile_id, actor_id and role. Roles are exactly admin, dokter, pasien and come from the account database. Never derive permissions from user_metadata or stored UI roles. Backend Auth session lookup is mandatory outside the test harness. Tokens require signature, issuer SUPABASE_URL/auth/v1, audience authenticated, integer iat/exp and age <=3600 seconds. HS256 is an optional legacy mode requiring a secret of at least 32 bytes. JWKS keys expire after 60 seconds; unavailable refresh fails closed.

| Resource | Patient | Doctor | Admin |
|---|---|---|---|
| Profile reads | Self/assigned doctor | Self/assigned patients | All |
| Profile edits | Self | Self | All |
| Sessions/records/images/stream tickets | Own | Assigned patients | All |
| START/STOP | Own assigned device | Assigned patient's device | All |
| Doctor connection | Patient issues consent code | Consume code as self | Assign explicitly |
| Device assignment | Consume device code for self | Denied | Explicit assignment |
| Hide/delete/session metadata | Denied | Denied | Allowed |
| Clinical frame validation | Denied | Assigned patients | Denied; clinical role required |
| Impersonation | Denied | Assigned patient | Non-admin target |

All private routes authenticate by default. Files under /uploads require the same authorization. Pagination uses page>=1 and limit 1..100; frames returns {data,pagination}. Records defaults to the first 50 visible frames; frame_id, frame_number or latest=true selects a frame. Frame times/numbers remain original when earlier frames are hidden. Export emits only authorized visible, non-deleted frames.

Impersonation uses POST /api/admin/impersonate/:account_id or /api/doctors/impersonate/:account_id, then X-Impersonation-Id. It expires after 10 minutes, is bound to the original actor and resolves current database permissions on every request. DELETE /api/impersonation/:id revokes it. The original SDK token is retained; no replacement token is stored in localStorage. Mutation audit records actor/effective identity.

## Provisioning and pairing

Supabase signup/email confirmation precedes POST /api/auth/register_profile. The verified token email must match the profile email; self-provisioning permits only pasien/dokter. Repeating the same role is idempotent. Account insertion precedes patient/doctor insertion in one transaction. Administrator creation uses Supabase Admin API and compensates failed database provisioning. Partial external failures are logged for recovery; never fabricate success.

POST /api/patients/:id/pairing-code creates a patient consent code. An administrator creates device codes at POST /api/devices/:id/pairing-code. Codes expire after 300 seconds, are stored as SHA256 digests, resource-bound and consumed transactionally once. Plain patient/device IDs are not consent.

Device identity has three distinct fields: the immutable server-generated `devices.id`, the editable display `name`, and the reviewed firmware `hardware_id`. MQTT and imported payloads must match `hardware_id` exactly; display names never authorize or route data. Migration `2026102709` deliberately leaves historical hardware IDs null for operator review. Unmapped devices remain visible to the API with `mapping_status: "unmapped"`, but listeners and acquisition fail closed.

Creating a device requires `hardware_id`; editing preserves the existing mapping when the field is omitted. A successful save returns `saved: true` and either `connection_status: "connecting"` or `"unavailable"`. Neither state claims the device is online; online status requires fresh telemetry. Internal IDs and hardware IDs share a collision-protected lookup namespace.

## Recording state machine

~~~mermaid
stateDiagram-v2
 [*] --> requested: START with Idempotency-Key
 requested --> recording: device START ACK
 requested --> interrupted: command timeout/failure
 recording --> stopping: STOP
 stopping --> ended: STOP ACK matches last persisted frame
 stopping --> interrupted: command timeout/failure
 recording --> recording: validated frame committed
 ended --> [*]
 interrupted --> [*]
~~~

POST /api/devices/:id/command accepts {command:"START",patient_id} or {command:"STOP",session_id,patient_id}. Idempotency-Key is required and bound to actor/effective identity/request. Queuing success is not device confirmation. The UI polls server state and waits for ACK.

TLS MQTT topics:
- Configured exact topic: frame
- topic/command: {"command":"START|STOP","session_id":"ses_...","idempotency_key":"..."}
- topic/ack: {"command":"START|STOP","session_id":"ses_...","last_frame_number":123}

STOP ACK must follow all final frame messages on the same device/topic ordering and report the final persisted number (0 for no frames). Firmware must adopt the server-issued session ID and reset its per-session sequence to 1. Duplicates are rejected; gaps interrupt live recovery until reconciled. MQTT credentials are never returned to patients/doctors or browser database roles.

Frames require message_id, registered hardware device_id, exact session_id, numeric string frame_id, RFC3339 created_at, sampling_rate_hz 100..1000, duration_s, and ecg:{format:"samples_by_time",samples:[[I,II,III],...]}. Channels are finite millivolts, exactly three values, maximum 30 seconds/30000 samples. Duration matches count/rate. Live timestamp tolerance is 120 seconds. Missing prediction means UNKNOWN; no fabricated battery, HR, AI accuracy or QTc.

## Stream and ECG coordinates

POST /api/stream/ticket with session_id issues a one-use 30-second ticket. Connect /ws?ticket=... from an exact ALLOWED_ORIGINS origin. Stream routing is session-scoped. Active sockets reverify JWT/JWKS, Auth session, database role/ownership and impersonation every 15 seconds. Slow sends time out; bounded queues discard live delivery while retaining committed history. Limits include 128 simultaneous sockets, four per actor, 32 in-flight paced frames globally, four queued frames per device, and two heavy HTTP requests; overload returns a failure/recovery signal.

Each packet is {"type":"ecg_packet","content":"<exact serialized JSON string>","sha256_checksum":"<hex SHA256 of UTF-8 content>"}. Verify before parsing/using content. SHA256 detects corruption; TLS provides transport authentication.

Waveforms include sample rate/count/offset, frame number, original start time, duration and device/session IDs. Reconnect retrieves committed data. get_segment index is frame_number-1, independent of hidden array positions.

Shared FE reference coordinates: 8 logical units/mm, 25mm/s, 10mm/mV; six measured/derived limb leads; aVR = -(I+II)/2, with no second inversion. The canvas applies chosen speed/gain once, to samples and markers together. Calibration is a separate 1mV, 0.4s pulse. Physical display size needs screen calibration. Normalization displays relative amplitude. Bandpass is causal second-order high-pass 0.5Hz plus second-order low-pass 40Hz, recalculated for actual fs; it is a display filter, not a validated diagnostic instrument.
