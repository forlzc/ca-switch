import { useCallback, useEffect, useMemo, useState } from "react";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { useTranslation } from "react-i18next";
import { toast } from "@/lib/toast";
import { Button } from "@/components/ui/button";
import { Form, FormField, FormItem, FormMessage } from "@/components/ui/form";
import { providerSchema, type ProviderFormData } from "@/lib/schemas/provider";
import {
  buildLocalProxyRequestOverrides,
  formatRequestOverrideObject,
} from "@/lib/requestOverrides";
import type { AppId } from "@/lib/api";
import type {
  ProviderCategory,
  ProviderMeta,
  ClaudeApiFormat,
  ClaudeApiKeyField,
  EndpointCandidate,
} from "@/types";
import { CommonConfigEditor } from "./CommonConfigEditor";
import { BasicFormFields } from "./BasicFormFields";
import { ClaudeFormFields } from "./ClaudeFormFields";
import {
  getApiKeyFromConfig,
  hasApiKeyField,
} from "@/utils/providerConfigUtils";
import { mergeProviderMeta } from "@/utils/providerMetaUtils";
import {
  toClaudeReasoningEffortChoice,
  type ClaudeReasoningEffortChoice,
} from "./ClaudeReasoningEffortField";
import { useApiKeyState, useBaseUrlState } from "./hooks";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { CLAUDE_DEFAULT_CONFIG } from "./helpers/claudeDefaults";
import type { AppMode } from "@/types/proxy";

type LocalProxyRequestOverridesBuildResult = ReturnType<
  typeof buildLocalProxyRequestOverrides
>;

/** 从配置里读当前选定的上游模型（ANTHROPIC_MODEL）。 */
function readModelFromConfig(config: unknown): string {
  const env = (config as { env?: Record<string, unknown> } | undefined)?.env;
  return typeof env?.ANTHROPIC_MODEL === "string" ? env.ANTHROPIC_MODEL : "";
}

export interface ProviderFormProps {
  appId: AppId;
  providerId?: string;
  submitLabel: string;
  onSubmit: (values: ProviderFormValues) => Promise<void> | void;
  onCancel: () => void;
  onSubmittingChange?: (isSubmitting: boolean) => void;
  onSubmitReadyChange?: (isReady: boolean) => void;
  initialData?: {
    name?: string;
    websiteUrl?: string;
    notes?: string;
    settingsConfig?: Record<string, unknown>;
    category?: ProviderCategory;
    meta?: ProviderMeta;
    icon?: string;
    iconColor?: string;
  };
  showButtons?: boolean;
  isProxyTakeover?: boolean;
  /**
   * 从供应商页哪一格打开的，不传时按应用实际生效的模式。
   */
  modeView?: AppMode;
}

export function ProviderForm(props: ProviderFormProps) {
  return <ProviderFormFull {...props} />;
}

function ProviderFormFull({
  appId,
  providerId,
  submitLabel,
  onSubmit,
  onCancel,
  onSubmittingChange,
  initialData,
  showButtons = true,
}: ProviderFormProps) {
  const { t } = useTranslation();
  const isEditMode = Boolean(initialData);

  // 精简后没有预设选择，表单始终按「自定义」渲染。
  const selectedPresetId = isEditMode ? null : "custom";
  const category = isEditMode ? initialData?.category : "custom";
  const [isEndpointModalOpen, setIsEndpointModalOpen] = useState(false);
  const [draftCustomEndpoints, setDraftCustomEndpoints] = useState<string[]>(
    () => [],
  );
  const [endpointAutoSelect, setEndpointAutoSelect] = useState<boolean>(
    () => initialData?.meta?.endpointAutoSelect ?? true,
  );
  const [localIsFullUrl, setLocalIsFullUrl] = useState<boolean>(
    () => initialData?.meta?.isFullUrl ?? false,
  );
  const [model, setModel] = useState<string>(() =>
    readModelFromConfig(initialData?.settingsConfig),
  );

  useEffect(() => {
    if (!initialData) {
      setDraftCustomEndpoints([]);
    }
    setEndpointAutoSelect(initialData?.meta?.endpointAutoSelect ?? true);
    setLocalIsFullUrl(initialData?.meta?.isFullUrl ?? false);
    setClaudeReasoningEffort(
      toClaudeReasoningEffortChoice(initialData?.meta?.claudeReasoningEffort),
    );
    setCustomUserAgent(initialData?.meta?.customUserAgent ?? "");
    setLocalProxyHeadersOverride(
      formatRequestOverrideObject(
        initialData?.meta?.localProxyRequestOverrides?.headers,
      ),
    );
    setLocalProxyBodyOverride(
      formatRequestOverrideObject(
        initialData?.meta?.localProxyRequestOverrides?.body,
      ),
    );
    setModel(readModelFromConfig(initialData?.settingsConfig));
  }, [initialData]);

  const defaultValues: ProviderFormData = useMemo(
    () => ({
      name: initialData?.name ?? "",
      websiteUrl: initialData?.websiteUrl ?? "",
      notes: initialData?.notes ?? "",
      settingsConfig: initialData?.settingsConfig
        ? JSON.stringify(initialData.settingsConfig, null, 2)
        : CLAUDE_DEFAULT_CONFIG,
      icon: initialData?.icon ?? "",
      iconColor: initialData?.iconColor ?? "",
    }),
    [initialData],
  );

  const form = useForm<ProviderFormData>({
    resolver: zodResolver(providerSchema),
    defaultValues,
    mode: "onSubmit",
  });
  const { isSubmitting } = form.formState;

  const handleSettingsConfigChange = useCallback(
    (config: string) => {
      if (form.getValues("settingsConfig") === config) {
        return;
      }
      form.setValue("settingsConfig", config);
    },
    [form],
  );

  // Event handlers must read the current settingsConfig instead of rebuilding
  // JSON from a render-time snapshot: the JSON editor writes through
  // form.setValue, and a snapshot taken during render can predate those edits.
  const getSettingsConfig = useCallback(
    () => form.getValues("settingsConfig"),
    [form],
  );

  const [localApiKeyField, setLocalApiKeyField] = useState<ClaudeApiKeyField>(
    () => {
      if (initialData?.meta?.apiKeyField) return initialData.meta.apiKeyField;
      // Infer from existing config env
      const env = (initialData?.settingsConfig as Record<string, unknown>)
        ?.env as Record<string, unknown> | undefined;
      if (env?.ANTHROPIC_API_KEY !== undefined) return "ANTHROPIC_API_KEY";
      return "ANTHROPIC_AUTH_TOKEN";
    },
  );

  // 软校验：收集"业务约束"类问题（空值/缺项），由用户决定是否仍要保存
  const [softIssues, setSoftIssues] = useState<string[] | null>(null);
  const [pendingFormValues, setPendingFormValues] =
    useState<ProviderFormData | null>(null);
  const [
    pendingLocalProxyRequestOverridesResult,
    setPendingLocalProxyRequestOverridesResult,
  ] = useState<LocalProxyRequestOverridesBuildResult | null>(null);
  // 确认框走的提交路径绕过了 react-hook-form 的 isSubmitting，单独追踪
  const [isConfirmSubmitting, setIsConfirmSubmitting] = useState(false);

  useEffect(() => {
    onSubmittingChange?.(isSubmitting || isConfirmSubmitting);
  }, [isSubmitting, isConfirmSubmitting, onSubmittingChange]);

  const {
    apiKey,
    handleApiKeyChange,
    showApiKey: shouldShowApiKey,
  } = useApiKeyState({
    initialConfig: form.getValues("settingsConfig"),
    onConfigChange: handleSettingsConfigChange,
    getConfig: getSettingsConfig,
    selectedPresetId,
    category,
    appType: appId,
    apiKeyField: localApiKeyField,
  });

  const { baseUrl, handleClaudeBaseUrlChange } = useBaseUrlState({
    category,
    settingsConfig: form.getValues("settingsConfig"),
    onSettingsConfigChange: handleSettingsConfigChange,
    getSettingsConfig,
  });

  // 直接选择服务商提供的一个模型，写进 ANTHROPIC_MODEL；留空表示跟随请求里的模型名。
  const handleModelChange = useCallback(
    (next: string) => {
      setModel(next);
      try {
        const config = JSON.parse(getSettingsConfig() || "{}");
        if (!config.env) config.env = {};
        const trimmed = next.trim();
        if (trimmed) {
          config.env.ANTHROPIC_MODEL = trimmed;
        } else {
          delete config.env.ANTHROPIC_MODEL;
        }
        handleSettingsConfigChange(JSON.stringify(config, null, 2));
      } catch (err) {
        console.error("Failed to update model config:", err);
      }
    },
    [getSettingsConfig, handleSettingsConfigChange],
  );

  // Values used for submit-time validation must come from the current JSON
  // editor contents, not stale hook state from the last render.
  const readSubmitTimeCredentials = useCallback(() => {
    const raw = getSettingsConfig() || "{}";
    let parsedBaseUrl: string | null = null;
    try {
      const parsed = JSON.parse(raw) as { env?: Record<string, unknown> };
      const candidate = parsed.env?.ANTHROPIC_BASE_URL;
      parsedBaseUrl = typeof candidate === "string" ? candidate.trim() : "";
    } catch {
      // The JSON validator will report malformed input; retain hook state for
      // the auxiliary checks until the user fixes it.
      parsedBaseUrl = null;
    }
    return {
      baseUrl: parsedBaseUrl ?? baseUrl,
      apiKey: parsedBaseUrl === null ? apiKey : getApiKeyFromConfig(raw, appId),
    };
  }, [apiKey, appId, baseUrl, getSettingsConfig]);

  const [localApiFormat, setLocalApiFormat] = useState<ClaudeApiFormat>(() => {
    return initialData?.meta?.apiFormat ?? "anthropic";
  });

  const handleApiFormatChange = useCallback((format: ClaudeApiFormat) => {
    setLocalApiFormat(format);
  }, []);

  const handleApiKeyFieldChange = useCallback(
    (field: ClaudeApiKeyField) => {
      const prev = localApiKeyField;
      setLocalApiKeyField(field);

      // Swap the env key name in settingsConfig
      try {
        const raw = form.getValues("settingsConfig");
        const config = JSON.parse(raw || "{}");
        if (config?.env && prev in config.env) {
          const value = config.env[prev];
          delete config.env[prev];
          config.env[field] = value;
          const updated = JSON.stringify(config, null, 2);
          handleSettingsConfigChange(updated);
        }
      } catch {
        // ignore parse errors during editing
      }
    },
    [localApiKeyField, form, handleSettingsConfigChange],
  );

  const [claudeReasoningEffort, setClaudeReasoningEffort] =
    useState<ClaudeReasoningEffortChoice>(() =>
      toClaudeReasoningEffortChoice(initialData?.meta?.claudeReasoningEffort),
    );
  const [customUserAgent, setCustomUserAgent] = useState<string>(
    () => initialData?.meta?.customUserAgent ?? "",
  );
  const [localProxyHeadersOverride, setLocalProxyHeadersOverride] =
    useState<string>(() =>
      formatRequestOverrideObject(
        initialData?.meta?.localProxyRequestOverrides?.headers,
      ),
    );
  const [localProxyBodyOverride, setLocalProxyBodyOverride] = useState<string>(
    () =>
      formatRequestOverrideObject(
        initialData?.meta?.localProxyRequestOverrides?.body,
      ),
  );
  useEffect(() => {
    form.reset(defaultValues);
  }, [defaultValues, form]);

  const shouldApplyLocalProxyRequestOverrides = category !== "official";

  const handleSubmit = async (values: ProviderFormData) => {
    const overridesResult = shouldApplyLocalProxyRequestOverrides
      ? buildLocalProxyRequestOverrides(
          localProxyHeadersOverride,
          localProxyBodyOverride,
        )
      : {};
    if (overridesResult.error) {
      toast.error(
        t("providerForm.localProxyRequestOverridesInvalid", {
          defaultValue: `本地代理请求覆盖格式错误：${overridesResult.error}`,
          error: overridesResult.error,
        }),
      );
      return;
    }

    // 软性问题（业务约束，用户可选择仍要保存）
    const issues: string[] = [];

    // 供应商名空：A 类
    if (!values.name.trim()) {
      issues.push(
        t("providerForm.fillSupplierName", {
          defaultValue: "请填写供应商名称",
        }),
      );
    }

    const { baseUrl: submitBaseUrl, apiKey: submitApiKey } =
      readSubmitTimeCredentials();

    // 非官方供应商端点 / API Key 空：A 类
    if (category !== "official" && category !== "cloud_provider") {
      if (!submitBaseUrl.trim()) {
        issues.push(
          t("providerForm.endpointRequired", {
            defaultValue: "非官方供应商请填写 API 端点",
          }),
        );
      }
      if (!submitApiKey.trim()) {
        issues.push(
          t("providerForm.apiKeyRequired", {
            defaultValue: "非官方供应商请填写 API Key",
          }),
        );
      }
    }

    if (issues.length > 0) {
      // 弹确认框让用户决定是否仍要保存
      setSoftIssues(issues);
      setPendingFormValues(values);
      setPendingLocalProxyRequestOverridesResult(overridesResult);
      return;
    }

    await performSubmit(values, overridesResult);
  };

  const performSubmit = async (
    values: ProviderFormData,
    overridesResult: LocalProxyRequestOverridesBuildResult,
  ) => {
    if (overridesResult.error) {
      toast.error(
        t("providerForm.localProxyRequestOverridesInvalid", {
          defaultValue: `本地代理请求覆盖格式错误：${overridesResult.error}`,
          error: overridesResult.error,
        }),
      );
      return;
    }

    const settingsConfig = values.settingsConfig.trim();

    const payload: ProviderFormValues = {
      ...values,
      name: values.name.trim(),
      websiteUrl: values.websiteUrl?.trim() ?? "",
      settingsConfig,
    };

    if (!isEditMode && draftCustomEndpoints.length > 0) {
      const customEndpointsToSave: Record<
        string,
        import("@/types").CustomEndpoint
      > = draftCustomEndpoints.reduce(
        (acc, url) => {
          const now = Date.now();
          acc[url] = { url, addedAt: now, lastUsed: undefined };
          return acc;
        },
        {} as Record<string, import("@/types").CustomEndpoint>,
      );

      const mergedMeta = mergeProviderMeta(
        initialData?.meta,
        customEndpointsToSave,
      );
      if (mergedMeta !== undefined) {
        payload.meta = mergedMeta;
      }
    }

    const metaSource = payload.meta ?? initialData?.meta;
    const baseMeta: ProviderMeta | undefined = metaSource
      ? { ...metaSource }
      : undefined;
    // Existing-provider edits never own endpoint membership. The backend
    // rejects endpoint-bearing update payloads; add/remove/touch use their
    // dedicated commands and remain safe from stale form snapshots.
    if (isEditMode && baseMeta) {
      delete baseMeta.custom_endpoints;
    }

    const nextMeta: ProviderMeta = {
      ...(baseMeta ?? {}),
      endpointAutoSelect,
      claudeReasoningEffort:
        category !== "official" &&
        (localApiFormat === "openai_chat" ||
          localApiFormat === "openai_responses") &&
        claudeReasoningEffort !== "auto"
          ? claudeReasoningEffort
          : undefined,
      customUserAgent:
        category !== "official"
          ? customUserAgent.trim() || undefined
          : undefined,
      localProxyRequestOverrides: shouldApplyLocalProxyRequestOverrides
        ? overridesResult.overrides
        : undefined,
      apiFormat: category !== "official" ? localApiFormat : undefined,
      apiKeyField:
        category !== "official" &&
        localApiKeyField !== "ANTHROPIC_AUTH_TOKEN"
          ? localApiKeyField
          : undefined,
      isFullUrl:
        category !== "official" && localIsFullUrl ? true : undefined,
    };

    payload.meta = nextMeta;

    await onSubmit(payload);
  };

  const shouldShowSpeedTest =
    category !== "official" && category !== "cloud_provider";

  const shouldShowClaudeApiKeyLink =
    category === "cn_official" ||
    category === "aggregator" ||
    category === "third_party";
  const claudeWebsiteUrl = form.watch("websiteUrl") || "";
  const isClaudePartner = false;
  const claudePartnerPromotionKey: string | undefined = undefined;

  // 端点测速的候选只来自当前地址和已保存配置；没有内置预设端点。
  const speedTestEndpoints = useMemo<EndpointCandidate[]>(() => {
    const map = new Map<string, EndpointCandidate>();
    const add = (url?: string) => {
      if (!url) return;
      const sanitized = url.trim().replace(/\/+$/, "");
      if (!sanitized || map.has(sanitized)) return;
      map.set(sanitized, { url: sanitized, isCustom: false });
    };
    add(baseUrl);
    if (initialData && typeof initialData.settingsConfig === "object") {
      const env = (initialData.settingsConfig as { env?: Record<string, unknown> })
        .env;
      const initialUrl = env?.ANTHROPIC_BASE_URL;
      if (typeof initialUrl === "string") add(initialUrl);
    }
    return Array.from(map.values());
  }, [baseUrl, initialData]);

  const settingsConfigErrorField = (
    <FormField
      control={form.control}
      name="settingsConfig"
      render={() => (
        <FormItem className="space-y-0">
          <FormMessage />
        </FormItem>
      )}
    />
  );

  return (
    <>
      <Form {...form}>
        <form
          id="provider-form"
          onSubmit={form.handleSubmit(handleSubmit)}
          className="space-y-6"
        >
          <BasicFormFields form={form} />

          <ClaudeFormFields
            providerId={providerId}
            shouldShowApiKey={
              (category !== "cloud_provider" ||
                hasApiKeyField(form.getValues("settingsConfig"), "claude")) &&
              shouldShowApiKey(form.getValues("settingsConfig"), isEditMode)
            }
            apiKey={apiKey}
            onApiKeyChange={handleApiKeyChange}
            category={category}
            shouldShowApiKeyLink={shouldShowClaudeApiKeyLink}
            websiteUrl={claudeWebsiteUrl}
            isPartner={isClaudePartner}
            partnerPromotionKey={claudePartnerPromotionKey}
            shouldShowSpeedTest={shouldShowSpeedTest}
            baseUrl={baseUrl}
            onBaseUrlChange={handleClaudeBaseUrlChange}
            isEndpointModalOpen={isEndpointModalOpen}
            onEndpointModalToggle={setIsEndpointModalOpen}
            onCustomEndpointsChange={
              isEditMode ? undefined : setDraftCustomEndpoints
            }
            autoSelect={endpointAutoSelect}
            onAutoSelectChange={setEndpointAutoSelect}
            showEndpointTools
            shouldShowModelSelector={category !== "official"}
            model={model}
            onModelChange={handleModelChange}
            speedTestEndpoints={speedTestEndpoints}
            apiFormat={localApiFormat}
            onApiFormatChange={handleApiFormatChange}
            reasoningEffort={claudeReasoningEffort}
            onReasoningEffortChange={setClaudeReasoningEffort}
            apiKeyField={localApiKeyField}
            onApiKeyFieldChange={handleApiKeyFieldChange}
            isFullUrl={localIsFullUrl}
            onFullUrlChange={setLocalIsFullUrl}
            customUserAgent={customUserAgent}
            onCustomUserAgentChange={setCustomUserAgent}
            localProxyHeadersOverride={localProxyHeadersOverride}
            onLocalProxyHeadersOverrideChange={setLocalProxyHeadersOverride}
            localProxyBodyOverride={localProxyBodyOverride}
            onLocalProxyBodyOverrideChange={setLocalProxyBodyOverride}
          />

          <CommonConfigEditor
            value={form.getValues("settingsConfig")}
            onChange={handleSettingsConfigChange}
          />
          {settingsConfigErrorField}

          {showButtons && (
            <div className="flex justify-end gap-2">
              <Button variant="outline" type="button" onClick={onCancel}>
                {t("common.cancel")}
              </Button>
              <Button
                type="submit"
                disabled={isSubmitting || isConfirmSubmitting}
              >
                {submitLabel}
              </Button>
            </div>
          )}
        </form>
      </Form>

      <ConfirmDialog
        isOpen={softIssues !== null && softIssues.length > 0}
        variant="info"
        title={t("providerForm.softValidation.title", {
          defaultValue: "配置存在以下问题",
        })}
        message={
          (softIssues ?? []).map((issue) => `• ${issue}`).join("\n") +
          "\n\n" +
          t("providerForm.softValidation.hint", {
            defaultValue:
              "仍要保存吗？保存后切换此供应商时可能失败，可以之后再补全。",
          })
        }
        confirmText={t("providerForm.softValidation.saveAnyway", {
          defaultValue: "仍要保存",
        })}
        cancelText={t("common.cancel")}
        onConfirm={async () => {
          if (isConfirmSubmitting) return;
          const values = pendingFormValues;
          const overridesResult = pendingLocalProxyRequestOverridesResult;
          if (!values || !overridesResult) {
            setSoftIssues(null);
            setPendingFormValues(null);
            setPendingLocalProxyRequestOverridesResult(null);
            return;
          }
          setIsConfirmSubmitting(true);
          try {
            await performSubmit(values, overridesResult);
            setSoftIssues(null);
            setPendingFormValues(null);
            setPendingLocalProxyRequestOverridesResult(null);
          } catch (error) {
            console.error("[ProviderForm] soft-confirm submit failed:", error);
            // 保留确认框和 pending values，让用户可以重试或取消
          } finally {
            setIsConfirmSubmitting(false);
          }
        }}
        onCancel={() => {
          if (isConfirmSubmitting) return;
          setSoftIssues(null);
          setPendingFormValues(null);
          setPendingLocalProxyRequestOverridesResult(null);
        }}
      />
    </>
  );
}

export type ProviderFormValues = ProviderFormData & {
  presetId?: string;
  presetCategory?: ProviderCategory;
  isPartner?: boolean;
  meta?: ProviderMeta;
};
