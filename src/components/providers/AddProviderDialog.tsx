import { useCallback, useState } from "react";
import { useTranslation } from "react-i18next";
import { Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { FullScreenPanel } from "@/components/common/FullScreenPanel";
import { APP_DISPLAY_NAME } from "@/components/shell/AppGlyph";
import type { Provider, CustomEndpoint } from "@/types";
import type { AppId } from "@/lib/api";
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
    provider: Omit<Provider, "id"> & { providerKey?: string },
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

  const closeDialog = useCallback(() => {
    onOpenChange(false);
  }, [onOpenChange]);

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

      await onSubmit(providerData);
      closeDialog();
    },
    [onSubmit, closeDialog],
  );

  const footer = (
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
        disabled={isFormSubmitting}
      >
        {isFormSubmitting && <Loader2 className="h-4 w-4 animate-spin" />}
        {t("common.add")}
      </Button>
    </>
  );

  return (
    <FullScreenPanel
      isOpen={open}
      trackUnsavedChanges
      title={t("provider.addNewProvider")}
      subtitle={APP_DISPLAY_NAME[appId]}
      backLabel={t("provider.backToList")}
      onClose={closeDialog}
      footer={footer}
      contentClassName="pt-4"
    >
      <ProviderForm
        appId={appId}
        submitLabel={t("common.add")}
        onSubmit={handleSubmit}
        onCancel={closeDialog}
        onSubmittingChange={setIsFormSubmitting}
        showButtons={false}
      />
    </FullScreenPanel>
  );
}
