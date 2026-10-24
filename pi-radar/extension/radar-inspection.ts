import type { RadarForeground } from "./radar-client.ts";

/** Bounded evidence captured and identity-checked by Radar for one child subject. */
export type RadarInspection = {
	capturedAt: number;
	recentOutputTruncated: boolean;
	recentOutput?: string;
	foreground?: RadarForeground;
	activity?: string;
};
