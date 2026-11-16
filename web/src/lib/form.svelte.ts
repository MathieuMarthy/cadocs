import { ApiError } from "./api";
import { type Key, t } from "./i18n";
import { en } from "./i18n/en";

/** What a refusal says to a person. */
export function describe(error: unknown) {
  if (!(error instanceof ApiError)) return t("error.unknown");
  if (error.status === 429) {
    return error.retryAfter
      ? t("error.tooManyIn", { seconds: error.retryAfter })
      : t("error.tooMany");
  }
  return error.message.toLowerCase();
}

/** The state of a form: busy while it submits, then what the server refused. */
export class Form {
  busy = $state(false);
  problem = $state<string | null>(null);
  fields = $state<Record<string, string>>({});

  /** The server's code for a field, as the catalogue words it: `too_short` reads `too short`. */
  field(name: string) {
    const code = this.fields[name];
    if (code === undefined) return null;
    const key = `field.${code}`;
    return key in en ? t(key as Key) : code.replaceAll("_", " ");
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
