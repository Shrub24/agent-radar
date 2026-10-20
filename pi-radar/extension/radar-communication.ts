import type { HandoffInboxSerialize, HandoffSerialize, HandoffReceiptSerialize } from "./generated-contracts.ts";
import type { RadarClient, RadarResult } from "./radar-client.ts";
import type { RadarPublication } from "./radar-publication.ts";

export type RadarHandoffPort = {
	/** Publish to this managed execution's Radar-recorded parent. Retry an uncertain result with the same ID and bytes. */
	publish(request: {
		request_id: string;
		correlation_id?: string;
		summary: string;
		content: string;
	}): Promise<RadarResult<{ handoff: HandoffSerialize; created: boolean }>>;
	/** Durable inbox scan for this process's managed subject. Notifications are hints; callers can always rescan. */
	inbox(): Promise<RadarResult<HandoffInboxSerialize>>;
	/** Retrieve the full immutable content even after it has been acknowledged. */
	get(handoff_id: string): Promise<RadarResult<{ handoff: HandoffSerialize; receipt: HandoffReceiptSerialize | null }>>;
	/** Call only after the consumer durably recorded the retrieved payload. */
	ack(handoff_id: string): Promise<RadarResult<{ receipt: HandoffReceiptSerialize; created: boolean }>>;
};

/**
 * Bind handoff operations to this Pi process's Radar registration and the
 * parent subject injected from its accepted managed-launch metadata.
 */
export function createRadarHandoffPort(
	client: RadarClient,
	publication: Pick<RadarPublication, "subject">,
	parentSubjectId: () => string | undefined = () => process.env.PI_RADAR_MANAGED_PARENT_SUBJECT_ID,
): RadarHandoffPort {
	async function agentId(): Promise<RadarResult<string>> {
		const subject = await publication.subject();
		return subject === undefined
			? { ok: false, code: "absent", message: "this process has no current Radar registration" }
			: { ok: true, value: subject.registration_id };
	}

	return {
		async publish(request) {
			const identity = await agentId();
			if (identity.ok === false) return identity;
			const parent = parentSubjectId();
			if (parent === undefined)
				return { ok: false, code: "absent", message: "this managed execution has no Radar parent subject" };
			return client.communicationPublish({
				...request,
				sender_registration_id: identity.value,
				recipient_subject_id: parent,
			});
		},
		async inbox() {
			const identity = await agentId();
			if (identity.ok === false) return identity;
			return client.communicationInbox({ registration_id: identity.value });
		},
		async get(handoff_id) {
			const identity = await agentId();
			if (identity.ok === false) return identity;
			return client.communicationGet({ registration_id: identity.value, handoff_id });
		},
		async ack(handoff_id) {
			const identity = await agentId();
			if (identity.ok === false) return identity;
			return client.communicationAck({ registration_id: identity.value, handoff_id });
		},
	};
}
