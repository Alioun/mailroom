import { z } from "zod";
import { agentProposal } from "../agent-exchange";
import { localFetch } from "../api";
import type { SummaryBinding } from "../api";

const binding = z.object({
  as_of: z.number(),
  version: z.string(),
  approved: z.array(z.string()),
  receipt: z.string(),
});
const request = z.object({
  id: z.string(),
  summary: binding,
  summary_sha256: z.string(),
  phase: z.enum([
    "prepared",
    "launching",
    "launched",
    "connected",
    "proposal_ready",
    "failed",
  ]),
  thread_id: z.string().nullable(),
  thread_link: z.string().nullable(),
  error: z.string().nullable(),
  proposals: z.array(
    z.object({
      id: z.string(),
      proposal: agentProposal,
      dismissed: z.boolean(),
    }),
  ),
});
const config = z.object({ provider: z.string(), model: z.string() });
const inbox = z.object({ requests: z.array(request), config: config.optional() });
export type AssistantConfig = z.infer<typeof config>;
export type AssistantRequest = z.infer<typeof request>;
export type AssistantInbox = z.infer<typeof inbox>;
type Command =
  | { action: "status" }
  | { action: "config_get" }
  | { action: "config_set"; provider: string; model: string }
  | { action: "prepare"; summary: SummaryBinding; confirmed: true }
  | { action: "launch"; request_id: string }
  | { action: "dismiss"; request_id: string; proposal_id: string };
export async function assistantAction(
  account: string,
  command: Command,
): Promise<AssistantInbox> {
  const response = await localFetch("/api/accounts/assistant", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ account, command }),
  });
  const value: unknown = await response.json();
  if (!response.ok) {
    const error = z.object({ error: z.string() }).safeParse(value);
    throw new Error(error.success ? error.data.error : "Assistant unavailable");
  }
  return inbox.parse(value);
}
export async function getAssistantConfig(account: string): Promise<AssistantConfig> {
  const response = await localFetch("/api/accounts/assistant", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ account, command: { action: "config_get" } }),
  });
  const value: unknown = await response.json();
  if (!response.ok) throw new Error("Assistant config unavailable");
  return z.object({ config }).parse(value).config;
}
export async function saveAssistantConfig(
  account: string,
  provider: string,
  model: string,
): Promise<AssistantConfig> {
  const response = await localFetch("/api/accounts/assistant", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      account,
      command: { action: "config_set", provider, model },
    }),
  });
  const value: unknown = await response.json();
  if (!response.ok) {
    const error = z.object({ error: z.string() }).safeParse(value);
    throw new Error(error.success ? error.data.error : "Invalid assistant config");
  }
  return z.object({ config }).parse(value).config;
}
