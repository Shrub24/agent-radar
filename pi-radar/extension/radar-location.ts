import { basename } from "node:path";
import type {
	RadarClient,
	RadarInventory,
	RadarLocation,
	RadarObservedPane,
} from "./radar-client.ts";

/**
 * What this process can say about its own location right now.
 *
 * `absent` is a fact about this process: it is not in a pane of the daemon's
 * backend, so it registers with no location on purpose. `pending` is not a fact
 * but a gap - the daemon or its topology did not answer for this process yet -
 * so nothing immutable may be written from it, and the next attempt asks again.
 */
export type RadarLocationResolution =
	| { state: "resolved"; location: RadarLocation }
	| { state: "absent"; reason: string }
	| { state: "pending"; reason: string };

/**
 * The pane this process's own environment reports: tmux sets `TMUX_PANE` for
 * every pane it creates, so it is the pane the terminal placed this process in,
 * never one found by searching for a name, a label, a session or a neighbour.
 */
export function radarLocalPane(env: NodeJS.ProcessEnv): string | undefined {
	const pane = env.TMUX_PANE?.trim();
	// The daemon parses a tmux handle strictly, and a malformed pane would refuse
	// the whole registration, so an unusable value publishes no location instead.
	return pane && /^%\d+$/.test(pane) ? pane : undefined;
}

/**
 * The raw tmux pane (`%N`) inside a daemon-qualified pane handle
 * (`tmux:<server>:%N`). A child compares its own raw pane with the parent's
 * qualified handle through this, because only the daemon can qualify its own.
 */
export function radarPaneHandle(qualified: string): string {
	return qualified.slice(qualified.lastIndexOf(":") + 1);
}

/**
 * The tmux server this process's own client is attached to, when it names one.
 *
 * `TMUX` is `<socket>,<pid>,<session>` and the socket's basename is the server
 * name, so this is the one local fact that can contradict the server the daemon
 * was started for.
 */
export function tmuxServerName(env: NodeJS.ProcessEnv): string | undefined {
	const socket = env.TMUX?.trim().split(",")[0];
	return socket ? basename(socket) || undefined : undefined;
}

/**
 * The pane handle this process may publish, qualified the way the daemon's
 * topology names it, or the reason it has none of its own.
 *
 * Herdsman drives only the tmux backend. A tmux handle is qualified with the
 * negotiated server, and the process's own client must name that same server: a
 * daemon started for one server must never be told about a pane of another.
 */
function qualifiedOwnPane(
	env: NodeJS.ProcessEnv,
	backend: string,
	instance: string | undefined,
): { pane: string } | { absent: string } | { pending: string } {
	if (backend !== "tmux")
		return {
			absent: `the daemon serves the ${backend} backend, and Herdsman drives only tmux`,
		};
	if (instance === undefined || instance === "")
		return {
			pending: "the daemon did not name the tmux server it addresses",
		};
	const server = tmuxServerName(env);
	if (server === undefined)
		return {
			absent:
				"this process's own tmux client names no server, so it is not in the pane it would publish",
		};
	if (server !== instance)
		return {
			absent: `this process is attached to tmux server ${server}, not to ${instance}`,
		};
	const pane = radarLocalPane(env);
	if (pane === undefined)
		return {
			absent:
				"this process's own environment names no tmux pane of that server",
		};
	return { pane: `tmux:${instance}:${pane}` };
}

/**
 * The containment a successful observation reports for one exact pane.
 *
 * The pane row is the authoritative source: a tmux daemon reports its panes and
 * no agents at all, so a pane's own location is the only thing that can place it.
 * A tab row that contradicts it makes the observation ambiguous rather than
 * authoritative.
 */
function observedContainment(
	inventory: RadarInventory,
	paneId: string,
): { pane: RadarObservedPane } | { ambiguous: string } | undefined {
	const rows = inventory.panes.filter((pane) => pane.pane_id === paneId);
	if (rows.length === 0) return undefined;
	if (rows.length > 1) return { ambiguous: `topology reports pane ${paneId} twice` };
	const row = rows[0]!;
	const tab = inventory.tabs.find((candidate) => candidate.tab_id === row.tab_id);
	if (tab !== undefined && tab.workspace_id !== row.workspace_id)
		return {
			ambiguous: `pane ${paneId} is reported in ${row.workspace_id}, but its tab belongs to ${tab.workspace_id}`,
		};
	return { pane: row };
}

/**
 * The location this process may publish for its own pane, or why it has none.
 *
 * The backend and its server are the daemon's own negotiated facts
 * (`daemon.ping`), never an inherited environment value. The pane is this
 * process's own environment fact, and the daemon's canonical topology has to
 * report that exact pane before any location is published: an unreadable
 * observation, or one that does not contain the pane, is a gap in the evidence
 * rather than permission to fall back to the environment. That is what keeps a
 * tmux-only fleet honest, where the daemon's inventory reports panes and no
 * agents at all.
 *
 * A spawned child reads the created pane's own environment - the same fact a
 * lead publishes. The one value that cannot be this child's own is the pane the
 * launch was created beside, which the launch records: when the environment
 * names it, this process publishes no location rather than claiming its
 * parent's pane. Such a process also refuses to be a managed agent, so the two
 * rules agree on the one case they overlap.
 */
export async function resolveRadarProcessLocation(
	client: RadarClient,
	env: NodeJS.ProcessEnv = process.env,
): Promise<RadarLocationResolution> {
	let ready: Awaited<ReturnType<RadarClient["ping"]>>;
	try {
		ready = await client.ping();
	} catch {
		return { state: "pending", reason: "the daemon did not answer this process" };
	}
	if (ready.ok === false)
		return {
			state: "pending",
			reason: `the daemon did not answer this process (${ready.code})`,
		};
	const backend = ready.value.backend;
	if (backend === undefined || backend === "none")
		return { state: "absent", reason: "the daemon serves no physical backend" };
	const instance = ready.value.instance ?? undefined;
	const own = qualifiedOwnPane(env, backend, instance);
	if ("absent" in own) return { state: "absent", reason: own.absent };
	if ("pending" in own) return { state: "pending", reason: own.pending };
	const paneId = own.pane;
	if (paneId === env.PI_HERDSMAN_PARENT_PANE_ID?.trim())
		return {
			state: "absent",
			reason:
				"this process is in the pane its launch was created beside, not in a pane of its own",
		};
	let inventory: Awaited<ReturnType<RadarClient["observe"]>>;
	try {
		inventory = await client.observe();
	} catch {
		return {
			state: "pending",
			reason: "the daemon's topology could not be read for this process's pane",
		};
	}
	if (inventory.ok === false)
		return {
			state: "pending",
			reason: `the daemon's topology could not be read (${inventory.code})`,
		};
	const containment = observedContainment(inventory.value, paneId);
	if (containment === undefined)
		return {
			state: "pending",
			reason: `the daemon's topology does not report pane ${paneId} yet`,
		};
	if ("ambiguous" in containment)
		return { state: "pending", reason: containment.ambiguous };
	const row = containment.pane;
	return {
		state: "resolved",
		location: {
			backend,
			...(instance !== undefined ? { instance } : {}),
			workspace: row.workspace_id,
			tab: row.tab_id,
			pane: paneId,
		},
	};
}
