import { useCallback, useState } from "react";
import { useTranslation } from "react-i18next";
import { Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { APP_DISPLAY_NAME } from "@/components/shell/AppGlyph";
import { FullScreenPanel } from "@/components/common/FullScreenPanel";
import type { Provider } from "@/types";
import type { AppMode } from "@/types/proxy";
import {
  ProviderForm,
  type ProviderFormValues,
} from "@/components/providers/forms/ProviderForm";
import type { AppId } from "@/lib/api";

interface EditProviderDialogProps {
  open: boolean;
  provider: Provider | null;
  onOpenChange: (open: boolean) => void;
  onSubmit: (payload: {
    provider: Provider;
    originalId?: string;
  }) => Promise<void> | void;
  appId: AppId;
  /** 保留旧签名：编辑页不再读取 live 配置。 */
  isProxyTakeover?: boolean;
  /** 保留旧签名：页头不再提示写配置文件。 */
  isCurrent?: boolean;
  /** 从供应商页哪一格打开的，见 ProviderForm 的同名参数 */
  modeView?: AppMode;
}

export function EditProviderDialog({
  open,
  provider,
  onOpenChange,
  onSubmit,
  appId,
}: EditProviderDialogProps) {
  const { t } = useTranslation();
  const [isFormSubmitting, setIsFormSubmitting] = useState(false);

  const closeDialog = useCallback(() => {
    onOpenChange(false);
  }, [onOpenChange]);

  const handleSubmit = useCallback(
    async (values: ProviderFormValues) => {
      if (!provider) return;

      const parsedConfig = JSON.parse(values.settingsConfig) as Record<
        string,
        unknown
      >;

      const updatedProvider: Provider = {
        ...provider,
        id: provider.id,
        name: values.name.trim(),
        notes: values.notes?.trim() || undefined,
        websiteUrl: values.websiteUrl?.trim() || undefined,
        settingsConfig: parsedConfig,
        icon: values.icon?.trim() || undefined,
        iconColor: values.iconColor?.trim() || undefined,
        ...(values.presetCategory ? { category: values.presetCategory } : {}),
        ...(values.meta ? { meta: values.meta } : {}),
      };

      await onSubmit({ provider: updatedProvider, originalId: provider.id });
      closeDialog();
    },
    [onSubmit, closeDialog, provider],
  );

  const initialData = provider
    ? {
        name: provider.name,
        notes: provider.notes,
        websiteUrl: provider.websiteUrl,
        settingsConfig: provider.settingsConfig as Record<string, unknown>,
        category: provider.category,
        meta: provider.meta,
        icon: provider.icon,
        iconColor: provider.iconColor,
      }
    : null;

  if (!provider || !initialData) {
    return null;
  }

  return (
    <FullScreenPanel
      isOpen={open}
      trackUnsavedChanges
      title={t("provider.editProviderNamed", { name: provider.name })}
      subtitle={APP_DISPLAY_NAME[appId]}
      backLabel={t("provider.backToList")}
      onClose={closeDialog}
      contentClassName="pt-4"
      footer={
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
            {t("common.save")}
          </Button>
        </>
      }
    >
      <ProviderForm
        appId={appId}
        providerId={provider.id}
        submitLabel={t("common.save")}
        onSubmit={handleSubmit}
        onCancel={closeDialog}
        onSubmittingChange={setIsFormSubmitting}
        initialData={initialData}
        showButtons={false}
      />
    </FullScreenPanel>
  );
}
