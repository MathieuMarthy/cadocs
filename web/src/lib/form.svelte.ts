import { ApiError } from "./api";

/** What a refusal says to a person. */
export function describe(error: unknown) {
  if (!(error instanceof ApiError)) return "something went wrong";
  if (error.status === 429) {
    return error.retryAfter
      ? `too many attempts, retry in ${error.retryAfter}s`
      : "too many attempts, retry later";
  }
  return error.message.toLowerCase();
}

/** The state of a form: busy while it submits, then what the server refused. */
export class Form {
  busy = $state(false);
  problem = $state<string | null>(null);
  fields = $state<Record<string, string>>({});

  /** `too_short` reads as `too short`. */
  field(name: string) {
    return this.fields[name]?.replaceAll("_", " ") ?? null;
  }

  /** Runs `action`. It returns whether it worked, and `explain` can word a refusal. */
  async run(action: () => Promise<unknown>, explain = describe) {
    if (this.busy) return false;
    this.busy = true;
    this.problem = null;
    this.fields = {};
    try {
      await action();
      return true;
    } catch (error) {
      this.fields = error instanceof ApiError ? error.fields : {};
      if (Object.keys(this.fields).length === 0) this.problem = explain(error);
      return false;
    } finally {
      this.busy = false;
    }
  }
}
