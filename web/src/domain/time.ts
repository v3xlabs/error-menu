const RELATIVE = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });

const UNITS: readonly (readonly [Intl.RelativeTimeFormatUnit, number])[] = [
  ["year", 365 * 24 * 60 * 60 * 1000],
  ["month", 30 * 24 * 60 * 60 * 1000],
  ["day", 24 * 60 * 60 * 1000],
  ["hour", 60 * 60 * 1000],
  ["minute", 60 * 1000],
];

export const sinceNow = (iso: string): string => {
  const elapsedMs = new Date(iso).getTime() - Date.now();
  const unit = UNITS.find(([, sizeMs]) => Math.abs(elapsedMs) >= sizeMs);

  if (unit === undefined) return RELATIVE.format(0, "second");

  return RELATIVE.format(Math.round(elapsedMs / unit[1]), unit[0]);
};

const DAY_MS = 24 * 60 * 60 * 1000;

// Calendar days, not elapsed hours: a job at 23:50 yesterday belongs to yesterday.
export const datePeriod = (iso: string, nowMs = Date.now()): string => {
  const startOfDay = (ms: number): number => new Date(ms).setHours(0, 0, 0, 0);
  const days = Math.round((startOfDay(nowMs) - startOfDay(Date.parse(iso))) / DAY_MS);

  if (days < 7) return RELATIVE.format(-days, "day");

  if (days < 30) return RELATIVE.format(-Math.floor(days / 7), "week");

  return RELATIVE.format(-Math.floor(days / 30), "month");
};
