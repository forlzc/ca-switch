import { useTranslation } from "react-i18next";
import { useQueryClient } from "@tanstack/react-query";
import { getVersion } from "@tauri-apps/api/app";
import { Copy, Sparkles } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { useSettingsQuery } from "@/lib/query";
import { useProxyStatusQuery } from "@/lib/query/proxy";
import { settingsApi } from "@/lib/api";
import { toast } from "@/lib/toast";
import { markSeen } from "@/lib/whatsNew";

const DEFAULT_PROXY_PORT = 15721;

/**
 * 首启向导：说明工作方式，并展示四条入口地址。
 *
 * 这里只展示地址与配置方式，不写任何客户端文件——caswitch 只提供协议入口，
 * 客户端配置由用户自己维护。
 */
export function FirstRunNoticeDialog() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const { data: settings } = useSettingsQuery();
  const { data: proxyStatus } = useProxyStatusQuery();

  // 后端启动时已经决定好要不要弹：条件不满足的话字段会立即被写成 true，
  // 所以前端这里只需要判空即可——与其他既有确认标记的模式一致。
  const isOpen = settings != null && settings.firstRunNoticeConfirmed !== true;

  const host = proxyStatus?.address || "127.0.0.1";
  const port = proxyStatus?.port || DEFAULT_PROXY_PORT;
  const base = `http://${host}:${port}`;

  const entries = [
    {
      key: "claude",
      label: t("firstRunNotice.entryClaude"),
      url: base,
      hint: t("firstRunNotice.entryClaudeHint"),
    },
    {
      key: "codex",
      label: t("firstRunNotice.entryCodex"),
      url: `${base}/v1`,
      hint: t("firstRunNotice.entryCodexHint"),
    },
    {
      key: "chat",
      label: t("firstRunNotice.entryChat"),
      url: `${base}/v1`,
      hint: t("firstRunNotice.entryChatHint"),
    },
    {
      key: "gemini",
      label: t("firstRunNotice.entryGemini"),
      url: `${base}/v1beta`,
      hint: t("firstRunNotice.entryGeminiHint"),
    },
  ];

  const handleAcknowledge = async () => {
    if (!settings) return;
    try {
      const { webdavSync: _, ...rest } = settings;
      // 新装用户看的就是新界面、新版本，「界面改版了」弹窗和更新摘要一并记成已看过
      const version = await getVersion().catch(() => undefined);
      await settingsApi.save({
        ...rest,
        firstRunNoticeConfirmed: true,
        newLayoutNoticeConfirmed: true,
        whatsNewSeenVersion: markSeen(settings.whatsNewSeenVersion, version),
      });
      await queryClient.invalidateQueries({ queryKey: ["settings"] });
    } catch (error) {
      console.error("Failed to save firstRunNoticeConfirmed:", error);
    }
  };

  return (
    <Dialog
      open={isOpen}
      onOpenChange={(open) => {
        if (!open) void handleAcknowledge();
      }}
    >
      <DialogContent className="max-w-xl" zIndex="top">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <Sparkles className="h-5 w-5 text-fg-1" />
            {t("firstRunNotice.title")}
          </DialogTitle>
        </DialogHeader>
        <div className="space-y-3 px-6 py-5">
          <DialogDescription className="whitespace-pre-line leading-relaxed">
            {t("firstRunNotice.bodyDefault")}
          </DialogDescription>
          <DialogDescription className="whitespace-pre-line leading-relaxed">
            {t("firstRunNotice.bodyOfficial")}
          </DialogDescription>

          <div className="space-y-2 rounded-lg border border-border bg-subtle/40 p-3">
            <div className="text-xs font-medium text-fg-2">
              {t("firstRunNotice.entriesTitle")}
            </div>
            {entries.map((entry) => (
              <div
                key={entry.key}
                className="flex items-center gap-2 rounded border border-border bg-background px-2 py-1.5"
              >
                <div className="min-w-0 flex-1">
                  <div className="text-xs font-medium text-fg-1">
                    {entry.label}
                    {entry.hint ? (
                      <span className="ml-1 font-normal text-fg-3">
                        {entry.hint}
                      </span>
                    ) : null}
                  </div>
                  <div className="truncate font-mono text-[11px] text-fg-2">
                    {entry.url}
                  </div>
                </div>
                <button
                  type="button"
                  className="rounded border border-border px-2 py-0.5 text-[11px] text-fg-1 hover:bg-subtle"
                  onClick={() => {
                    void navigator.clipboard
                      ?.writeText(entry.url)
                      .then(() =>
                        toast.success(t("firstRunNotice.copied")),
                      );
                  }}
                  aria-label={t("firstRunNotice.copy")}
                >
                  <Copy className="h-3.5 w-3.5" />
                </button>
              </div>
            ))}
            <div className="text-[11px] text-fg-3">
              {t("firstRunNotice.entriesHint")}
            </div>
          </div>
        </div>
        <DialogFooter>
          <Button onClick={handleAcknowledge}>
            {t("firstRunNotice.confirm")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
