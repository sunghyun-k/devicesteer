// The banner that offers an update found at launch (agents.ts).

import { Icon } from "../../components/Icon";
import { useT } from "../../i18n/react";
import { dismissUpdate, installUpdate, useAgents } from "./agents";

export function UpdateBanner({ className }: { className: string }) {
  const t = useT();
  const update = useAgents((a) => a.update);
  if (!update) return null;
  return (
    <div id="update-banner" className={className} role="status">
      <Icon name="i-download" />
      <p id="update-banner-text">{update.text ?? t("update.available", { version: update.update.version })}</p>
      <button id="update-install" className="btn small" type="button" disabled={update.busy} onClick={() => void installUpdate()}>
        {t("update.restart")}
      </button>
      <button id="update-dismiss" className="icon-btn small" type="button" aria-label={t("common.dismiss")} title={t("common.dismiss")} onClick={dismissUpdate}>
        <Icon name="i-close" />
      </button>
    </div>
  );
}
