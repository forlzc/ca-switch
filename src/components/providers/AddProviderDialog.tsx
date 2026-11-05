import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { FullScreenPanel } from "@/components/common/FullScreenPanel";
import { APP_DISPLAY_NAME } from "@/components/shell/AppGlyph";
import type { Provider, CustomEndpoint } from "@/types";
import type { AppId } from "@/lib/api";
import { providersApi } from "@/lib/api";
import type {
  EditorConflictPolicy,
  ProviderEditorSave,
} from "@/lib/api/providers";
import { useLiveEditConflict } from "@/components/providers/LiveEditConflictDialog";
import { toastEditorViewFailed } from "@/components/providers/forms/hooks/useDraftEditorProjection";
import { isPiFamilyApp } from "@/config/appConfig";
import {
  ProviderForm,
  type ProviderFormValues,
} from "@/components/providers/forms/ProviderForm";
import type { AppMode } from "@/types/proxy";

interface AddProviderDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  appId: AppId;
  onSubmit: (
    provider: Omit<Provider, "id"> & {
      providerKey?: string;
      editorSave?: ProviderEditorSave;
    },
  ) => Promise<void> | void;
  /** 兼容旧签名；精简后不再有直连 / 聚合布局。 */
  modeView?: AppMode;
}

/**
 * 新增供应商：不再有预设选择，打开即进入自定义配置表单。
 */
export function AddProviderDialog({
  open,
  onOpenChange,
  appId,
  onSubmit,
}: AddProviderDialogProps) {
  const { t } = useTranslation();
  const [isFormSubmitting, setIsFormSubmitting] = useState(false);
  const [accountLogin, setAccountLogin] = useState(false);

  // Claude：把自定义配置套在当前 live 上显示，保存时其余部分的改动写进 live。
  const [claudeLiveBase, setClaudeLiveBase] = useState<Record<
    string,
    unknown
  > | null>(null);
  const [claudeBaseLoaded, setClaudeBaseLoaded] = useState(false);
  const { submitWithConflictRetry, conflictDialog } = useLiveEditConflict();

  useEffect(() => {
    setAccountLogin(false);
  }, [appId, open]);

  useEffect(() => {
    if (!open || appId !== "claude") {
      setClaudeLiveBase(null);
      setClaudeBaseLoaded(false);
      return;
    }
    let cancelled = false;
    providersApi
      .getEditorView(appId, {})
      .then((view) => {
        if (!cancelled) setClaudeLiveBase(view.settings);
      })
      .catch((error: unknown) => {
        if (!cancelled) {
          setClaudeLiveBase(null);
          toastEditorViewFailed(t, error);
        }
      })
      .finally(() => {
        if (!cancelled) setClaudeBaseLoaded(true);
      });
    return () => {
      cancelled = true;
    };
  }, [open, appId, t]);

  const closeDialog = useCallback(() => {
    onOpenChange(false);
  }, [onOpenChange]);

  const formReadyToken = useMemo(() => Symbol("provider-form-ready"), [appId, open]);
  const [formReadyState, setFormReadyState] = useState({
    token: formReadyToken,
    ready: !isPiFamilyApp(appId),
  });
  const isFormReady =
    formReadyState.token === formReadyToken
      ? formReadyState.ready
      : !isPiFamilyApp(appId);

  const handleSubmitReadyChange = useCallback(
    (ready: boolean) => {
      setFormReadyState({ token: formReadyToken, ready });
    },
    [formReadyToken],
  );

  const handleSubmit = useCallback(
    async (values: ProviderFormValues) => {
      const parsedConfig = JSON.parse(values.settingsConfig) as Record<
        string,
        unknown
      >;

      const providerData: Omit<Provider, "id"> = {
        name: values.name.trim(),
        notes: values.notes?.trim() || undefined,
        websiteUrl: values.websiteUrl?.trim() || undefined,
        settingsConfig: parsedConfig,
        icon: values.icon?.trim() || undefined,
        iconColor: values.iconColor?.trim() || undefined,
        ...(values.presetCategory ? { category: values.presetCategory } : {}),
        ...(values.meta ? { meta: values.meta } : {}),
      };

      const env = parsedConfig.env as Record<string, unknown> | undefined;
      const baseUrl =
        typeof env?.ANTHROPIC_BASE_URL === "string"
          ? env.ANTHROPIC_BASE_URL.trim().replace(/\/+$/, "")
          : "";
      if (baseUrl.startsWith("http")) {
        const customEndpoints: Record<string, CustomEndpoint> = {
          [baseUrl]: {
            url: baseUrl,
            addedAt: Date.now(),
            lastUsed: undefined,
          },
        };
        providerData.meta = {
          ...(providerData.meta ?? {}),
          custom_endpoints: customEndpoints,
        };
      }

      const editorBase = claudeLiveBase && { base: claudeLiveBase };
      const submit = async (onConflict: EditorConflictPolicy) => {
        await onSubmit({
          ...providerData,
          ...(editorBase ? { editorSave: { ...editorBase, onConflict } } : {}),
        });
        closeDialog();
      };
      await submitWithConflictRetry(submit);
    },
    [onSubmit, closeDialog, claudeLiveBase, submitWithConflictRetry],
  );

  const waitingForClaudeBase = appId === "claude" && !claudeBaseLoaded;

  const footer = !accountLogin ? (
    <>
      <Button
        type="button"
        variant="neutral"
        size="regular"
        onClick={closeDialog}
      >
        {t("common.cancel")}
      </Button>
      <Button
        type="submit"
        form="provider-form"
        variant="solid"
        size="regular"
        disabled={isFormSubmitting || !isFormReady}
      >
        {isFormSubmitting && <Loader2 className="h-4 w-4 animate-spin" />}
        {t("common.add")}
      </Button>
    </>
  ) : null;

  const form = (
    <ProviderForm
      appId={appId}
      submitLabel={t("common.add")}
      onSubmit={handleSubmit}
      onCancel={closeDialog}
      onSubmittingChange={setIsFormSubmitting}
      onSubmitReadyChange={handleSubmitReadyChange}
      onAccountLoginChange={setAccountLogin}
      showButtons={false}
      claudeLiveBase={
        appId === "claude" ? (claudeLiveBase ?? undefined) : undefined
      }
    />
  );

  return (
    <FullScreenPanel
      isOpen={open}
      trackUnsavedChanges={!accountLogin}
      title={t("provider.addNewProvider")}
      subtitle={APP_DISPLAY_NAME[appId]}
      backLabel={t("provider.backToList")}
      onClose={closeDialog}
      footer={footer}
      contentClassName={isPiFamilyApp(appId) ? "pb-0 pt-4" : "pt-4"}
    >
      {waitingForClaudeBase ? (
        <div className="py-12 text-center text-body text-fg-2">
          {t("common.loading")}
        </div>
      ) : (
        form
      )}
      {conflictDialog}
    </FullScreenPanel>
  );
}
