import { createFetch } from "openapi-hooks";

import type { paths } from "./schema.gen";

export const api = createFetch<paths>({ baseUrl: `${globalThis.location.origin}/api/` });
