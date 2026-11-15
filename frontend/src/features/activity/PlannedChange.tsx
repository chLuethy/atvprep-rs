import { For, Show, type Component } from "solid-js";
import { t } from "../../lib/i18n";
import { plannedStepTexts } from "../files/fixPlan";
import type { PlannedChange as Change } from "../../lib/encode";
import type { PlanStep } from "../files/planSteps";
import { changeLabel } from "../files/preparePlan";

/**
 * What a Vorbereiten item was expected to change when it was queued: the Änderungsart as a small
 * mark and the planned steps below it. An item queued without one (older entries, Prüfen,
 * Analysieren) shows nothing.
 */
export const PlannedChange: Component<{ change?: Change | null | undefined; steps?: ReadonlyArray<PlanStep> | null | undefined }> = (props) => {
  const steps = () => plannedStepTexts(props.steps ?? []);
  return (
    <Show when={props.change}>
      {(change) => (
        <div class="item-plan" role="group" aria-label={t("Expected change")}>
          <span class={`change-mark ${change()}`}>{changeLabel(change())}</span>
          <Show when={steps().length > 0}>
            <ul class="plain item-steps" aria-label={t("Planned steps")}>
              <For each={steps()}>{(step) => <li>{step}</li>}</For>
            </ul>
          </Show>
        </div>
      )}
    </Show>
  );
};
