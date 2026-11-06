import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { HelpTip } from "@/components/ui/help-tip";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import { toast } from "@/lib/toast";
import { FormLabel } from "@/components/ui/form";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { ChevronDown, ChevronRight } from "lucide-react";
import EndpointSpeedTest from "./EndpointSpeedTest";
import {
  ApiKeySection,
  EndpointField,
  FullUrlToggle,
  ModelInputWithFetch,
} from "./shared";
import {
  fetchModelsForConfig,
  modelFetchRequestHeaders,
  showFetchModelsError,
  type FetchedModel,
} from "@/lib/api/model-fetch";
import { CustomUserAgentField } from "./CustomUserAgentField";
import { LocalProxyRequestOverridesField } from "./LocalProxyRequestOverridesField";
import {
  ClaudeReasoningEffortField,
  type ClaudeReasoningEffortChoice,
} from "./ClaudeReasoningEffortField";
import type {
  ProviderCategory,
  ClaudeApiFormat,
  ClaudeApiKeyField,
} from "@/types";

interface EndpointCandidate {
  url: string;
}

interface ClaudeFormFieldsProps {
  providerId?: string;
  // API Key
  shouldShowApiKey: boolean;
  apiKey: string;
  onApiKeyChange: (key: string) => void;
  category?: ProviderCategory;
  shouldShowApiKeyLink: boolean;
  websiteUrl: string;
  isPartner?: boolean;
  partnerPromotionKey?: string;

  // Base URL
  shouldShowSpeedTest: boolean;
  baseUrl: string;
  onBaseUrlChange: (url: string) => void;
  isEndpointModalOpen: boolean;
  onEndpointModalToggle: (open: boolean) => void;
  onCustomEndpointsChange?: (endpoints: string[]) => void;
  autoSelect: boolean;
  onAutoSelectChange: (checked: boolean) => void;
  showEndpointTools?: boolean;

  // Model selection：直接从服务商的 /models 里选一个模型
  shouldShowModelSelector: boolean;
  model: string;
  onModelChange: (model: string) => void;

  // Speed Test Endpoints
  speedTestEndpoints: EndpointCandidate[];

  // API Format (for Claude-compatible providers that need request/response conversion)
  apiFormat: ClaudeApiFormat;
  onApiFormatChange: (format: ClaudeApiFormat) => void;
  // 转成 OpenAI Chat / Responses 时思考档位的发法
  reasoningEffort: ClaudeReasoningEffortChoice;
  onReasoningEffortChange: (value: ClaudeReasoningEffortChoice) => void;

  // Auth Field (ANTHROPIC_AUTH_TOKEN or ANTHROPIC_API_KEY)
  apiKeyField: ClaudeApiKeyField;
  onApiKeyFieldChange: (field: ClaudeApiKeyField) => void;

  // Full URL mode
  isFullUrl: boolean;
  onFullUrlChange: (value: boolean) => void;

  // Local proxy User-Agent override
  customUserAgent: string;
  onCustomUserAgentChange: (value: string) => void;
  localProxyHeadersOverride: string;
  onLocalProxyHeadersOverrideChange: (value: string) => void;
  localProxyBodyOverride: string;
  onLocalProxyBodyOverrideChange: (value: string) => void;
}

export function ClaudeFormFields({
  providerId,
  shouldShowApiKey,
  apiKey,
  onApiKeyChange,
  category,
  shouldShowApiKeyLink,
  websiteUrl,
  isPartner,
  partnerPromotionKey,
  shouldShowSpeedTest,
  baseUrl,
  onBaseUrlChange,
  isEndpointModalOpen,
  onEndpointModalToggle,
  onCustomEndpointsChange,
  autoSelect,
  onAutoSelectChange,
  showEndpointTools = true,
  shouldShowModelSelector,
  model,
  onModelChange,
  speedTestEndpoints,
  apiFormat,
  onApiFormatChange,
  reasoningEffort,
  onReasoningEffortChange,
  apiKeyField,
  onApiKeyFieldChange,
  isFullUrl,
  onFullUrlChange,
  customUserAgent,
  onCustomUserAgentChange,
  localProxyHeadersOverride,
  onLocalProxyHeadersOverrideChange,
  localProxyBodyOverride,
  onLocalProxyBodyOverrideChange,
}: ClaudeFormFieldsProps) {
  const { t } = useTranslation();
  const hasRequestOverrides = Boolean(
    localProxyHeadersOverride.trim() || localProxyBodyOverride.trim(),
  );
  const hasAnyAdvancedValue = !!(
    apiFormat !== "anthropic" ||
    reasoningEffort !== "auto" ||
    apiKeyField !== "ANTHROPIC_AUTH_TOKEN" ||
    isFullUrl ||
    customUserAgent ||
    hasRequestOverrides
  );
  const [advancedExpanded, setAdvancedExpanded] = useState(hasAnyAdvancedValue);

  // 预设填充高级值后自动展开（仅从折叠→展开，不会自动折叠）
  useEffect(() => {
    if (hasAnyAdvancedValue) {
      setAdvancedExpanded(true);
    }
  }, [hasAnyAdvancedValue]);

  // 通用模型获取
  const [fetchedModels, setFetchedModels] = useState<FetchedModel[]>([]);
  const [isFetchingModels, setIsFetchingModels] = useState(false);

  const handleFetchModels = useCallback(() => {
    if (!baseUrl || !apiKey) {
      showFetchModelsError(null, t, {
        hasApiKey: !!apiKey,
        hasBaseUrl: !!baseUrl,
      });
      return;
    }
    setIsFetchingModels(true);
    fetchModelsForConfig(
      baseUrl,
      apiKey,
      isFullUrl,
      undefined,
      customUserAgent,
      {
        requestHeaders: modelFetchRequestHeaders(localProxyHeadersOverride),
      },
    )
      .then((models) => {
        setFetchedModels(models);
        if (models.length === 0) {
          toast.info(t("providerForm.fetchModelsEmpty"));
        } else {
          toast.success(
            t("providerForm.fetchModelsSuccess", { count: models.length }),
          );
        }
      })
      .catch((err) => {
        console.warn("[ModelFetch] Failed:", err);
        showFetchModelsError(err, t);
      })
      .finally(() => setIsFetchingModels(false));
  }, [
    baseUrl,
    apiKey,
    isFullUrl,
    customUserAgent,
    localProxyHeadersOverride,
    t,
  ]);

  const apiKeySection = (
    <ApiKeySection
      value={apiKey}
      onChange={onApiKeyChange}
      category={category}
      required
      shouldShowLink={shouldShowApiKeyLink}
      websiteUrl={websiteUrl}
      isPartner={isPartner}
      partnerPromotionKey={partnerPromotionKey}
    />
  );

  // 字段说明一律收进字段名后的「?」，页面上只留后果提示。
  const endpointHint =
    apiFormat === "openai_responses"
      ? t("providerForm.apiHintResponses")
      : apiFormat === "openai_chat"
        ? t("providerForm.apiHintOAI")
        : apiFormat === "gemini_native"
          ? t("providerForm.apiHintGeminiNative")
          : t("providerForm.apiHint");

  const endpointSection = shouldShowSpeedTest && (
    <EndpointField
      id="baseUrl"
      label={t("providerForm.apiEndpoint")}
      value={baseUrl}
      onChange={onBaseUrlChange}
      placeholder={t("providerForm.apiEndpointPlaceholder")}
      help={endpointHint}
      showManageButton={showEndpointTools}
      onManageClick={
        showEndpointTools ? () => onEndpointModalToggle(true) : undefined
      }
    />
  );

  const speedTestModal =
    shouldShowSpeedTest && showEndpointTools && isEndpointModalOpen ? (
      <EndpointSpeedTest
        appId="claude"
        providerId={providerId}
        value={baseUrl}
        onChange={onBaseUrlChange}
        initialEndpoints={speedTestEndpoints}
        visible={isEndpointModalOpen}
        onClose={() => onEndpointModalToggle(false)}
        autoSelect={autoSelect}
        onAutoSelectChange={onAutoSelectChange}
        onCustomEndpointsChange={onCustomEndpointsChange}
      />
    ) : null;

  const apiFormatLabel = t("providerForm.apiFormat", {
    defaultValue: "上游格式",
  });

  const apiFormatField = (
    <>
      {category !== "cloud_provider" && (
        <div className="space-y-2">
          <div className="flex items-center gap-0.5">
            <FormLabel htmlFor="apiFormat">{apiFormatLabel}</FormLabel>
            <HelpTip title={apiFormatLabel}>
              {t("providerForm.apiFormatHelp", {
                defaultValue:
                  "看供应商原生支持哪种协议：Anthropic Messages 原样转发、不转换格式；其余三种由路由转成 Anthropic Messages。",
              })}
            </HelpTip>
          </div>
          <Select value={apiFormat} onValueChange={onApiFormatChange}>
            <SelectTrigger id="apiFormat" className="w-full">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="anthropic">
                {t("providerForm.apiFormatAnthropic", {
                  defaultValue: "Anthropic Messages (原生)",
                })}
              </SelectItem>
              <SelectItem value="openai_chat">
                {t("providerForm.apiFormatOpenAIChat", {
                  defaultValue: "OpenAI Chat Completions (需转换)",
                })}
              </SelectItem>
              <SelectItem value="openai_responses">
                {t("providerForm.apiFormatOpenAIResponses", {
                  defaultValue: "OpenAI Responses API (需转换)",
                })}
              </SelectItem>
              <SelectItem value="gemini_native">
                {t("providerForm.apiFormatGeminiNative", {
                  defaultValue: "Gemini Native generateContent (需转换)",
                })}
              </SelectItem>
            </SelectContent>
          </Select>
        </div>
      )}
      {category !== "cloud_provider" &&
        (apiFormat === "openai_chat" || apiFormat === "openai_responses") && (
          <ClaudeReasoningEffortField
            id="claudeReasoningEffort"
            value={reasoningEffort}
            onChange={onReasoningEffortChange}
          />
        )}
    </>
  );

  // 认证字段选择器
  const authFieldLabel = t("providerForm.authField", {
    defaultValue: "认证字段",
  });
  const authFieldSelect = (
    <div className="space-y-2">
      <div className="flex items-center gap-0.5">
        <FormLabel>{authFieldLabel}</FormLabel>
        <HelpTip title={authFieldLabel}>
          {t("providerForm.authFieldHint", {
            defaultValue:
              "直连时是写进 settings.json 的变量名；经过路由时，AUTH_TOKEN 以 Bearer、API_KEY 以 x-api-key 发给上游。",
          })}
        </HelpTip>
      </div>
      <Select
        value={apiKeyField}
        onValueChange={(v) => onApiKeyFieldChange(v as ClaudeApiKeyField)}
      >
        <SelectTrigger>
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value="ANTHROPIC_AUTH_TOKEN">
            {t("providerForm.authFieldAuthToken", {
              defaultValue: "ANTHROPIC_AUTH_TOKEN（默认）",
            })}
          </SelectItem>
          <SelectItem value="ANTHROPIC_API_KEY">
            {t("providerForm.authFieldApiKey", {
              defaultValue: "ANTHROPIC_API_KEY",
            })}
          </SelectItem>
        </SelectContent>
      </Select>
    </div>
  );

  const fullUrlToggle = shouldShowSpeedTest && showEndpointTools && (
    <FullUrlToggle
      checked={isFullUrl}
      onCheckedChange={onFullUrlChange}
      geminiNative={apiFormat === "gemini_native"}
    />
  );

  // 两项都只在路由模式下生效：在分组开头说一次，各自的作用收进字段名后的「?」。
  const userAgentAndOverrides = (
    <>
      <p className="text-xs leading-relaxed text-fg-2">
        {t("codexConfig.routeOnlySettingsHint", {
          defaultValue: "以下两项只在路由模式下生效。",
        })}
      </p>
      <CustomUserAgentField
        id="claude-custom-user-agent"
        value={customUserAgent}
        onChange={onCustomUserAgentChange}
        help={t("providerForm.customUserAgentHelp", {
          defaultValue: "替换转发到供应商的请求里的 User-Agent。",
        })}
      />
      <div className="border-t border-border pt-3">
        <LocalProxyRequestOverridesField
          headersJson={localProxyHeadersOverride}
          bodyJson={localProxyBodyOverride}
          onHeadersJsonChange={onLocalProxyHeadersOverrideChange}
          onBodyJsonChange={onLocalProxyBodyOverrideChange}
          help={t("providerForm.localProxyRequestOverridesHelp", {
            defaultValue: "作用于格式转换后发给上游的请求。",
          })}
        />
      </div>
    </>
  );

  const modelLabel = t("providerForm.defaultModelLabel", {
    defaultValue: "模型",
  });

  return (
    <>
      {shouldShowApiKey && apiKeySection}
      {endpointSection}
      {speedTestModal}

      {shouldShowModelSelector && (
        <div className="space-y-2">
          <div className="flex items-center gap-0.5">
            <FormLabel htmlFor="claudeModel">{modelLabel}</FormLabel>
            <HelpTip title={modelLabel}>
              {t("providerForm.defaultModelHint", {
                defaultValue:
                  "从服务商提供的模型里选一个；留空则直接用请求里的模型名发给上游。",
              })}
            </HelpTip>
          </div>
          <ModelInputWithFetch
            id="claudeModel"
            value={model}
            onChange={onModelChange}
            placeholder={t("providerForm.modelPlaceholder", {
              defaultValue: "",
            })}
            fetchedModels={fetchedModels}
            isLoading={isFetchingModels}
            onFetch={handleFetchModels}
          />
          {!model.trim() && (
            <p className="text-xs leading-relaxed text-fg-2">
              {t("providerForm.defaultModelPassthrough", {
                defaultValue: "未设置模型，上游将收到请求里的模型名。",
              })}
            </p>
          )}
        </div>
      )}

      {shouldShowModelSelector && (
        <Collapsible
          open={advancedExpanded}
          onOpenChange={setAdvancedExpanded}
          className="rounded-lg border border-border p-4"
        >
          <CollapsibleTrigger asChild>
            <Button
              type="button"
              variant={null}
              size="sm"
              className="h-8 w-full justify-start gap-1.5 px-0 text-sm font-medium text-fg-1 hover:opacity-70 active:scale-100"
            >
              {advancedExpanded ? (
                <ChevronDown className="h-4 w-4" />
              ) : (
                <ChevronRight className="h-4 w-4" />
              )}
              {t("providerForm.advancedOptionsToggle")}
            </Button>
          </CollapsibleTrigger>
          <CollapsibleContent className="space-y-4 pt-2">
            {apiFormatField}

            {fullUrlToggle}

            {/* 认证字段只决定 Anthropic 格式上游的认证头（OpenAI 格式一律 Bearer） */}
            {apiFormat === "anthropic" && authFieldSelect}

            <div className="space-y-4 border-t border-border pt-4">
              {userAgentAndOverrides}
            </div>
          </CollapsibleContent>
        </Collapsible>
      )}
    </>
  );
}
