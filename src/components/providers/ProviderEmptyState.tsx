import { Plus, Server } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";

interface ProviderEmptyStateProps {
  onCreate?: () => void;
}

/**
 * 供应商页空状态：通用代理没有客户端配置可导入，只提供「添加供应商」。
 */
export function ProviderEmptyState({ onCreate }: ProviderEmptyStateProps) {
  const { t } = useTranslation();

  return (
    <div className="flex flex-col items-center justify-center px-6 py-16 text-center">
      <div className="mb-5 flex h-14 w-14 items-center justify-center rounded-2xl border border-border bg-subtle text-fg-2">
        <Server className="h-6 w-6" strokeWidth={1.5} aria-hidden="true" />
      </div>
      <h2 className="text-section text-fg-1">{t("provider.noProviders")}</h2>
      {onCreate && (
        <div className="mt-5 flex flex-wrap items-center justify-center gap-2">
          <Button variant="neutral" size="regular" onClick={onCreate}>
            <Plus className="h-4 w-4" />
            {t("provider.addProvider")}
          </Button>
        </div>
      )}
    </div>
  );
}
