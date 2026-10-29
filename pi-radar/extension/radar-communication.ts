import type {
	MessagePageSerialize,
	MessageReceiptSerialize,
	MessageSerialize,
} from "./generated-contracts.ts";
import type { RadarClient, RadarResult } from "./radar-client.ts";
import type { RadarPublication } from "./radar-publication.ts";

/**
 * A message body a consumer knows how to incorporate. `capture` runs before the
 * recipient receipt is recorded, so a crash between the two redelivers rather
 * than losing the body. Returning `false` leaves the message pending: the
 * consumer failed to record it durably.
 */
export type RadarMessageConsumer = {
	kind: MessageSerialize["body"]["kind"];
	capture(message: MessageSerialize): Promise<boolean> | boolean;
};

export type RadarMailboxPort = {
	/** Publish to this managed execution's Radar-recorded parent. Retry an uncertain result with the same ID and bytes. */
	publish(request: {
		request_id: string;
		correlation_id?: string;
		summary: string;
		content: string;
	}): Promise<RadarResult<{ message: MessageSerialize; created: boolean }>>;
	/** One bounded page of pending message ids for this process's managed subject. */
	page(after?: string): Promise<RadarResult<MessagePageSerialize>>;
	/** Retrieve full immutable content, including after it has been received. */
	get(message_id: string): Promise<RadarResult<{ message: MessageSerialize; receipt: MessageReceiptSerialize | null }>>;
	/** Call only after the consumer durably recorded the retrieved body. */
	receive(message_id: string): Promise<RadarResult<{ receipt: MessageReceiptSerialize; created: boolean }>>;
};

/**
 * Bind message operations to this Pi process's Radar registration and the parent
 * subject injected from its accepted managed-launch metadata.
 */
export function createRadarMailboxPort(
	client: RadarClient,
	publication: Pick<RadarPublication, "subject">,
	parentSubjectId: () => string | undefined = () => process.env.PI_RADAR_MANAGED_PARENT_SUBJECT_ID,
): RadarMailboxPort {
	async function registration(): Promise<RadarResult<string>> {
		const subject = await publication.subject();
		return subject === undefined
			? { ok: false, code: "absent", message: "this process has no current Radar registration" }
			: { ok: true, value: subject.registration_id };
	}

	return {
		async publish(request) {
			const identity = await registration();
			if (identity.ok === false) return identity;
			const parent = parentSubjectId();
			if (parent === undefined)
				return { ok: false, code: "absent", message: "this managed execution has no Radar parent subject" };
			return client.messagePublish({
				...request,
				sender_registration_id: identity.value,
				recipient_subject_id: parent,
			});
		},
		async page(after) {
			const identity = await registration();
			if (identity.ok === false) return identity;
			return client.messagePage({
				registration_id: identity.value,
				...(after === undefined ? {} : { after }),
			});
		},
		async get(message_id) {
			const identity = await registration();
			if (identity.ok === false) return identity;
			return client.messageGet({ registration_id: identity.value, message_id });
		},
		async receive(message_id) {
			const identity = await registration();
			if (identity.ok === false) return identity;
			return client.messageReceive({ registration_id: identity.value, message_id });
		},
	};
}
