import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "@/lib/toast";
import { invoke } from "@tauri-apps/api/core";
import { useQueryClient } from "@tanstack/react-query";
import { MoreHorizontal, Plus, Search } from "lucide-react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { Provider, VisibleApps } from "@/types";
import type { EnvConflict } from "@/types/env";
import { useProvidersQuery, useSettingsQuery } from "@/lib/query";
import {
  providersApi,
  settingsApi,
  type AppId,
  type ProviderSwitchEvent,
} from "@/lib/api";
import { checkAllEnvConflicts, checkEnvConflicts } from "@/lib/api/env";
import { useProviderActions } from "@/hooks/useProviderActions";

import {
  useTrayAppPageSeen,
  useTrayNavigation,
} from "@/hooks/useTrayNavigation";
import { useTauriEvent } from "@/hooks/useTauriEvent";
import { useLastValidValue } from "@/hooks/useLastValidValue";
import { useUsageCacheBridge } from "@/hooks/useUsageCacheBridge";
import { extractErrorMessage } from "@/utils/errorUtils";
import { isTextEditableTarget } from "@/utils/domUtils";
import { deepClone } from "@/utils/deepClone";
import { isLinux, isMac, isWindows } from "@/lib/platform";
import {
  APP_STORAGE_KEY,
  readStoredView,
  storeView,
  type GlobalPage,
  type SettingsSection,
  type View,
} from "@/lib/navigation";
import { Sidebar } from "@/components/shell/Sidebar";
import { useUpdate } from "@/contexts/UpdateContext";
import { NewLayoutDialog } from "@/components/shell/NewLayoutDialog";
import {
  AppPageHeader,
  WindowControlsContext,
} from "@/components/shell/AppPageHeader";
import { WindowControls } from "@/components/shell/WindowControls";
import { APP_DISPLAY_NAME, AppGlyph } from "@/components/shell/AppGlyph";
import { ProfileSwitcher } from "@/components/profiles/ProfileSwitcher";
import { AddProviderDialog } from "@/components/providers/AddProviderDialog";
import { EditProviderDialog } from "@/components/providers/EditProviderDialog";
import { CustomProviderPanel } from "@/components/providers/CustomProviderPanel";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { discardUnsavedChanges, hasUnsavedChanges } from "@/lib/unsavedChanges";
import { SettingsPage } from "@/components/settings/SettingsPage";
import { UsagePage } from "@/components/usage/UsagePage";
import { EnvWarningBanner } from "@/components/env/EnvWarningBanner";
import UsageScriptModal from "@/components/UsageScriptModal";
import { DeepLinkImportDialog } from "@/components/DeepLinkImportDialog";
import { FirstRunNoticeDialog } from "@/components/FirstRunNoticeDialog";
import { WhatsNewNotice } from "@/components/WhatsNewDialog";
import { Button } from "@/components/ui/button";
import { HoverTip } from "@/components/ui/hover-tip";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { DEFAULT_VISIBLE_APPS } from "@/config/appConfig";

interface SyncStatusUpdatedPayload {
  source?: string;
  status?: string;
  error?: string;
}

/** 精简后只管理 Claude 一个客户端。 */
const ACTIVE_APP: AppId = "claude";

function App() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const { hasUpdate } = useUpdate();

  const [currentView, setCurrentView] = useState<View>(readStoredView);
  const [settingsSection, setSettingsSection] =
    useState<SettingsSection>("general");
  // 进设置前停留的页面：设置目录里的「← 返回」回到这里
  const settingsReturnViewRef = useRef<View>("providers");
  const [isAddOpen, setIsAddOpen] = useState(false);
  const [providerSearchOpen, setProviderSearchOpen] = useState(false);
  useEffect(() => {
    setProviderSearchOpen(false);
  }, [currentView]);

  useEffect(() => {
    storeView(currentView);
  }, [currentView]);

  const { data: settingsData } = useSettingsQuery();
  const useAppWindowControls =
    isWindows() || (isLinux() && (settingsData?.useAppWindowControls ?? false));
  const visibleApps = useMemo<VisibleApps>(
    () => ({
      ...DEFAULT_VISIBLE_APPS,
      ...settingsData?.visibleApps,
      // 唯一的 API 管理入口，不能被旧设置隐藏
      claude: true,
    }),
    [settingsData?.visibleApps],
  );

  const [editingProvider, setEditingProvider] = useState<Provider | null>(null);
  const [usageProvider, setUsageProvider] = useState<Provider | null>(null);
  const [confirmProvider, setConfirmProvider] = useState<Provider | null>(null);
  const [envConflicts, setEnvConflicts] = useState<EnvConflict[]>([]);
  const [showEnvBanner, setShowEnvBanner] = useState(false);

  const effectiveEditingProvider = useLastValidValue(editingProvider);
  const effectiveUsageProvider = useLastValidValue(usageProvider);
  const mainScrollRef = useRef<HTMLElement>(null);
  const providerScrollContainerRef = useRef<HTMLDivElement>(null);

  useUsageCacheBridge();

  useLayoutEffect(() => {
    if (currentView !== "providers") return;

    for (const container of [
      mainScrollRef.current,
      providerScrollContainerRef.current,
    ]) {
      if (container) {
        container.scrollTop = 0;
        container.scrollLeft = 0;
      }
    }
  }, [currentView]);

  const { data, isLoading, refetch } = useProvidersQuery(ACTIVE_APP);
  const providers = useMemo(() => data?.providers ?? {}, [data]);
  const currentProviderId = data?.currentProviderId ?? "";

  const { addProvider, updateProvider, switchProvider, deleteProvider, saveUsageScript } =
    useProviderActions(ACTIVE_APP, false);

  useEffect(() => {
    let unsubscribe: (() => void) | undefined;
    let active = true;

    const setupListener = async () => {
      try {
        const off = await providersApi.onSwitched(
          async (event: ProviderSwitchEvent) => {
            await queryClient.invalidateQueries({
              queryKey: ["providers", event.appType],
            });
          },
        );
        if (!active) {
          off();
          return;
        }
        unsubscribe = off;
      } catch (error) {
        console.error("[App] Failed to subscribe provider switch event", error);
      }
    };

    void setupListener();
    return () => {
      active = false;
      unsubscribe?.();
    };
  }, [queryClient]);

  useTauriEvent("universal-provider-synced", async () => {
    await queryClient.invalidateQueries({ queryKey: ["providers"] });
    try {
      await providersApi.updateTrayMenu();
    } catch (error) {
      console.error("[App] Failed to update tray menu", error);
    }
  });

  useTauriEvent<SyncStatusUpdatedPayload | null | undefined>(
    "webdav-sync-status-updated",
    async (payload) => {
      const statusPayload = payload ?? {};
      await queryClient.invalidateQueries({ queryKey: ["settings"] });
      if (statusPayload.source !== "auto" || statusPayload.status !== "error") {
        return;
      }
      toast.error(
        t("settings.webdavSync.autoSyncFailedToast", {
          error: statusPayload.error || t("common.unknown"),
        }),
      );
    },
  );

  useTauriEvent<SyncStatusUpdatedPayload | null | undefined>(
    "s3-sync-status-updated",
    async (payload) => {
      const statusPayload = payload ?? {};
      await queryClient.invalidateQueries({ queryKey: ["settings"] });
      if (statusPayload.source !== "auto" || statusPayload.status !== "error") {
        return;
      }
      toast.error(
        t("settings.s3Sync.autoSyncFailedToast", {
          error: statusPayload.error || t("common.unknown"),
        }),
      );
    },
  );

  useEffect(() => {
    if (!settingsData) return;

    const syncWindowDecorations = async () => {
      try {
        await getCurrentWindow().setDecorations(!useAppWindowControls);
      } catch (error) {
        console.error("[App] Failed to update window decorations", error);
      }
    };

    void syncWindowDecorations();
  }, [useAppWindowControls, settingsData]);

  useEffect(() => {
    if (!isLinux() || !useAppWindowControls) return;

    const win = getCurrentWindow();
    let active = true;
    let revision = 0;
    let canResize = false;
    let nativeScale = 1;
    const unlisteners: (() => void)[] = [];
    let cursor:
      | {
          target: HTMLElement | SVGElement;
          value: string;
          priority: string;
          applied: string;
        }
      | undefined;
    const restoreCursor = () => {
      if (
        cursor &&
        cursor.target.style.cursor === cursor.applied &&
        cursor.target.style.getPropertyPriority("cursor") === "important"
      ) {
        cursor.target.style.setProperty(
          "cursor",
          cursor.value,
          cursor.priority,
        );
      }
      cursor = undefined;
    };
    const refresh = async () => {
      const current = ++revision;
      canResize = false;
      restoreCursor();
      try {
        const [maximized, fullscreen, resizable, scale] = await Promise.all([
          win.isMaximized(),
          win.isFullscreen(),
          win.isResizable(),
          win.scaleFactor(),
        ]);
        if (active && current === revision) {
          canResize = resizable && !maximized && !fullscreen;
          nativeScale = scale;
        }
      } catch (error) {
        console.error("[App] Failed to read window resize state", error);
      }
    };
    const edgeCursor = ({ clientX: x, clientY: y }: MouseEvent) => {
      if (
        !canResize ||
        x < 0 ||
        y < 0 ||
        x >= window.innerWidth ||
        y >= window.innerHeight
      ) {
        return "";
      }
      const inset = (5 * nativeScale * nativeScale) / window.devicePixelRatio;
      const horizontal =
        x < inset ? -1 : x >= window.innerWidth - inset ? 1 : 0;
      const vertical = y < inset ? -1 : y >= window.innerHeight - inset ? 1 : 0;
      if (horizontal && vertical) {
        return horizontal === vertical ? "nwse-resize" : "nesw-resize";
      }
      return horizontal ? "ew-resize" : vertical ? "ns-resize" : "";
    };
    const onMouseMove = (event: MouseEvent) => {
      const value = edgeCursor(event);
      const target = event.target;
      if (
        !value ||
        !(target instanceof HTMLElement || target instanceof SVGElement)
      ) {
        restoreCursor();
        return;
      }
      if (cursor?.target !== target) {
        restoreCursor();
        cursor = {
          target,
          value: target.style.cursor,
          priority: target.style.getPropertyPriority("cursor"),
          applied: "",
        };
      }
      if (cursor.applied !== value) {
        cursor.applied = value;
        target.style.setProperty("cursor", value, "important");
      }
    };
    const onMouseDown = (event: MouseEvent) => {
      if (!(event.target instanceof Element)) return;
      const drag = event.target.getAttribute("data-tauri-drag-region");
      if (
        event.button === 0 &&
        drag !== null &&
        drag !== "false" &&
        edgeCursor(event)
      ) {
        event.stopPropagation();
      }
    };

    document.addEventListener("mousemove", onMouseMove, true);
    document.addEventListener("mousedown", onMouseDown, true);
    document.documentElement.addEventListener("mouseleave", restoreCursor);
    window.addEventListener("blur", restoreCursor);
    for (const listener of [
      win.onResized(refresh),
      win.onScaleChanged(refresh),
    ]) {
      void listener
        .then((unlisten) => {
          if (active) unlisteners.push(unlisten);
          else unlisten();
        })
        .catch((error) =>
          console.error("[App] Failed to listen for window resize", error),
        );
    }
    void refresh();
    return () => {
      active = false;
      unlisteners.forEach((unlisten) => unlisten());
      document.removeEventListener("mousemove", onMouseMove, true);
      document.removeEventListener("mousedown", onMouseDown, true);
      document.documentElement.removeEventListener("mouseleave", restoreCursor);
      window.removeEventListener("blur", restoreCursor);
      restoreCursor();
    };
  }, [useAppWindowControls]);

  useEffect(() => {
    const checkEnvOnStartup = async () => {
      try {
        const allConflicts = await checkAllEnvConflicts();
        const flatConflicts = Object.values(allConflicts).flat();

        if (flatConflicts.length > 0) {
          setEnvConflicts(flatConflicts);
          const dismissed = sessionStorage.getItem("env_banner_dismissed");
          if (!dismissed) {
            setShowEnvBanner(true);
          }
        }
      } catch (error) {
        console.error(
          "[App] Failed to check environment conflicts on startup:",
          error,
        );
      }
    };

    checkEnvOnStartup();
  }, []);

  useEffect(() => {
    const checkMigration = async () => {
      try {
        const migrated = await invoke<boolean>("get_migration_result");
        if (migrated) {
          toast.success(
            t("migration.success", { defaultValue: "配置迁移成功" }),
            { closeButton: true },
          );
        }
      } catch (error) {
        console.error("[App] Failed to check migration result:", error);
      }
    };

    checkMigration();
  }, [t]);

  useEffect(() => {
    const checkEnvOnSwitch = async () => {
      try {
        const conflicts = await checkEnvConflicts(ACTIVE_APP);

        if (conflicts.length > 0) {
          setEnvConflicts((prev) => {
            const existingKeys = new Set(
              prev.map((c) => `${c.varName}:${c.sourcePath}`),
            );
            const newConflicts = conflicts.filter(
              (c) => !existingKeys.has(`${c.varName}:${c.sourcePath}`),
            );
            return [...prev, ...newConflicts];
          });
          const dismissed = sessionStorage.getItem("env_banner_dismissed");
          if (!dismissed) {
            setShowEnvBanner(true);
          }
        }
      } catch (error) {
        console.error(
          "[App] Failed to check environment conflicts on app switch:",
          error,
        );
      }
    };

    void checkEnvOnSwitch();
  }, []);

  const currentViewRef = useRef(currentView);

  useEffect(() => {
    currentViewRef.current = currentView;
  }, [currentView]);

  // ─── 导航 ───────────────────────────────────────────────────────────────

  const closeProviderPanels = () => {
    setIsAddOpen(false);
    setEditingProvider(null);
    setUsageProvider(null);
  };

  // 整页编辑器里有未保存的修改：先确认，确认放弃后再执行这次导航
  const [pendingLeave, setPendingLeave] = useState<(() => void) | null>(null);
  const confirmLeave = (proceed: () => void) => {
    if (!hasUnsavedChanges()) return false;
    setPendingLeave(() => proceed);
    return true;
  };

  const openSettings = (section: SettingsSection = "general") => {
    if (confirmLeave(() => openSettings(section))) return;
    closeProviderPanels();
    if (currentViewRef.current !== "settings") {
      settingsReturnViewRef.current = currentViewRef.current;
    }
    setSettingsSection(section);
    setCurrentView("settings");
  };

  const exitSettings = () => {
    setCurrentView(settingsReturnViewRef.current);
  };

  const selectApp = (_app: AppId) => {
    if (confirmLeave(() => selectApp(_app))) return;
    closeProviderPanels();
    localStorage.setItem(APP_STORAGE_KEY, ACTIVE_APP);
    setCurrentView("providers");
  };

  const openPage = (page: GlobalPage | "settings") => {
    if (page === "settings") {
      openSettings("general");
      return;
    }
    if (currentViewRef.current === page) return;
    if (confirmLeave(() => openPage(page))) return;
    closeProviderPanels();
    setCurrentView(page);
  };

  useTrayAppPageSeen(currentView === "providers" ? ACTIVE_APP : null);

  useTrayNavigation((navigation) => {
    const navigate = () => {
      if (navigation.section) {
        openSettings(navigation.section);
        return;
      }
      selectApp(ACTIVE_APP);
      if (navigation.intent === "add") setIsAddOpen(true);
    };
    if (!confirmLeave(navigate)) navigate();
  });

  const openPageFromNav = (page: GlobalPage | "settings") => {
    // 「设置」上的绿点说的是 CA Switch 有新版本，点进去直接到「关于」里的更新按钮
    if (page === "settings" && hasUpdate) {
      openSettings("about");
      return;
    }
    openPage(page);
  };

  const openSettingsRef = useRef(openSettings);
  openSettingsRef.current = openSettings;

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      const mod = event.metaKey || event.ctrlKey;
      if (mod && event.key === ",") {
        event.preventDefault();
        openSettingsRef.current("general");
        return;
      }

      if (event.key !== "Escape" || event.defaultPrevented) return;
      if (document.body.style.overflow === "hidden") return;
      if (isTextEditableTarget(event.target)) return;

      const view = currentViewRef.current;
      if (view === "settings") {
        event.preventDefault();
        setCurrentView(settingsReturnViewRef.current);
      }
    };

    window.addEventListener("keydown", handleKeyDown);
    return () => {
      window.removeEventListener("keydown", handleKeyDown);
    };
  }, []);

  const handleOpenWebsite = async (url: string) => {
    try {
      await settingsApi.openExternal(url);
    } catch (error) {
      const detail =
        extractErrorMessage(error) ||
        t("notifications.openLinkFailed", {
          defaultValue: "链接打开失败",
        });
      toast.error(detail);
    }
  };

  const handleEditProvider = async ({
    provider,
    originalId,
  }: {
    provider: Provider;
    originalId?: string;
  }) => {
    await updateProvider(provider, originalId);
    setEditingProvider(null);
  };

  const handleConfirmDelete = async () => {
    if (!confirmProvider) return;
    try {
      await deleteProvider(confirmProvider.id);
    } catch {
      // useDeleteProviderMutation 的 onError 已经弹了错误 toast；确认框照样关掉
    }
    setConfirmProvider(null);
  };

  const handleDuplicateProvider = async (provider: Provider) => {
    const newSortIndex =
      provider.sortIndex !== undefined ? provider.sortIndex + 1 : undefined;

    if (newSortIndex !== undefined) {
      const updates = Object.values(providers)
        .filter(
          (p) =>
            p.sortIndex !== undefined &&
            p.sortIndex >= newSortIndex &&
            p.id !== provider.id,
        )
        .map((p) => ({ id: p.id, sortIndex: p.sortIndex! + 1 }));

      if (updates.length > 0) {
        try {
          await providersApi.updateSortOrder(updates, ACTIVE_APP);
        } catch (error) {
          console.error("[App] Failed to update sort order", error);
          toast.error(
            t("provider.sortUpdateFailed", {
              defaultValue: "排序更新失败",
            }),
          );
          return;
        }
      }
    }

    await addProvider({
      name: `${provider.name} copy`,
      settingsConfig: deepClone(provider.settingsConfig),
      websiteUrl: provider.websiteUrl,
      category: provider.category,
      sortIndex: newSortIndex,
      meta: provider.meta ? deepClone(provider.meta) : undefined,
      icon: provider.icon,
      iconColor: provider.iconColor,
    });
  };

  const handleImportSuccess = async () => {
    try {
      await queryClient.invalidateQueries({
        queryKey: ["providers"],
        refetchType: "all",
      });
      await queryClient.refetchQueries({
        queryKey: ["providers"],
        type: "all",
      });
    } catch (error) {
      console.error("[App] Failed to refresh providers after import", error);
      await refetch();
    }
    try {
      await providersApi.updateTrayMenu();
    } catch (error) {
      console.error("[App] Failed to refresh tray menu", error);
    }
  };

  const openAddProvider = () => setIsAddOpen(true);

  // ─── 应用页 ─────────────────────────────────────────────────────────────

  const appMenu = (
    <DropdownMenu>
      <HoverTip content={t("common.more")}>
        <DropdownMenuTrigger asChild>
          <Button
            variant="quiet"
            size="icon-compact"
            className="h-8 w-8"
            aria-label={t("appPage.moreActions", {
              name: APP_DISPLAY_NAME[ACTIVE_APP],
            })}
          >
            <MoreHorizontal className="h-4 w-4" />
          </Button>
        </DropdownMenuTrigger>
      </HoverTip>
      <DropdownMenuContent align="end" className="min-w-[180px]">
        <DropdownMenuItem onSelect={() => openPage("usage")}>
          {t("appPage.viewUsage")}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );

  const renderAppPageHeader = () => (
    <AppPageHeader
      variant="app"
      icon={<AppGlyph size={20} />}
      title={APP_DISPLAY_NAME[ACTIVE_APP]}
      actions={
        <>
          {currentView === "providers" &&
            (settingsData?.showProfileSwitcher ?? false) && (
              <ProfileSwitcher activeApp={ACTIVE_APP} />
            )}
          {currentView === "providers" &&
            (settingsData?.showProviderSearch ?? true) && (
              <HoverTip
                content={t("provider.searchButtonTip", {
                  shortcut: isMac() ? "⌘F" : "Ctrl+F",
                })}
              >
                <Button
                  variant="quiet"
                  size="icon-compact"
                  className="h-8 w-8"
                  aria-label={t("provider.searchAriaLabel")}
                  aria-pressed={providerSearchOpen}
                  onClick={() => setProviderSearchOpen((open) => !open)}
                >
                  <Search className="h-4 w-4" />
                </Button>
              </HoverTip>
            )}
          {currentView === "providers" && (
            <Button
              variant="solid"
              size="regular"
              onClick={openAddProvider}
            >
              <Plus className="h-4 w-4" />
              {t("provider.addProvider")}
            </Button>
          )}
          {appMenu}
        </>
      }
    />
  );

  const listCallbacks = {
    onEdit: (provider: Provider) => setEditingProvider(provider),
    onDelete: (provider: Provider) => setConfirmProvider(provider),
    onDuplicate: handleDuplicateProvider,
    onConfigureUsage: setUsageProvider,
    onOpenWebsite: handleOpenWebsite,
    onCreate: openAddProvider,
    searchOpen: providerSearchOpen,
    onSearchOpenChange: setProviderSearchOpen,
  };

  const renderProviderList = () => (
    <CustomProviderPanel
      providers={providers}
      currentProviderId={currentProviderId}
      isLoading={isLoading}
      scrollRef={providerScrollContainerRef}
      onSwitch={switchProvider}
      {...listCallbacks}
    />
  );

  const renderAppPage = () => (
    <>
      {renderAppPageHeader()}
      {renderProviderList()}
    </>
  );

  // ─── 全局页 ─────────────────────────────────────────────────────────────

  const renderGlobalPage = () => {
    if (currentView === "usage") {
      return (
        <UsagePage onOpenRoutingSettings={() => openSettings("routing")} />
      );
    }
    return null;
  };

  const renderContent = () => {
    if (currentView === "settings") {
      return (
        <SettingsPage
          section={settingsSection}
          onImportSuccess={handleImportSuccess}
        />
      );
    }
    return currentView === "providers" ? renderAppPage() : renderGlobalPage();
  };

  return (
    <WindowControlsContext.Provider
      value={useAppWindowControls ? <WindowControls /> : null}
    >
      <div className="flex h-screen overflow-hidden bg-app text-fg-1 selection:bg-action/25">
        <Sidebar
          activeApp={ACTIVE_APP}
          view={currentView}
          visibleApps={visibleApps}
          settingsSection={settingsSection}
          onSelectApp={selectApp}
          onSelectPage={openPageFromNav}
          onSelectSettingsSection={setSettingsSection}
          onExitSettings={exitSettings}
        />
        <main
          ref={mainScrollRef}
          id="content-area"
          className="relative isolate flex min-w-0 flex-1 flex-col overflow-hidden bg-app"
        >
          {showEnvBanner && envConflicts.length > 0 && (
            <EnvWarningBanner
              conflicts={envConflicts}
              onDismiss={() => {
                setShowEnvBanner(false);
                sessionStorage.setItem("env_banner_dismissed", "true");
              }}
              onDeleted={async () => {
                try {
                  const allConflicts = await checkAllEnvConflicts();
                  const flatConflicts = Object.values(allConflicts).flat();
                  setEnvConflicts(flatConflicts);
                  if (flatConflicts.length === 0) {
                    setShowEnvBanner(false);
                  }
                } catch (error) {
                  console.error(
                    "[App] Failed to re-check conflicts after deletion:",
                    error,
                  );
                }
              }}
            />
          )}
          <div
            key={
              currentView === "settings" ? "settings" : currentView
            }
            className="flex min-h-0 flex-1 flex-col"
          >
            {renderContent()}
          </div>
        </main>
      </div>

      <AddProviderDialog
        open={isAddOpen}
        onOpenChange={setIsAddOpen}
        appId={ACTIVE_APP}
        onSubmit={addProvider}
      />

      <EditProviderDialog
        open={Boolean(editingProvider)}
        provider={effectiveEditingProvider}
        onOpenChange={(open) => {
          if (!open) {
            setEditingProvider(null);
          }
        }}
        onSubmit={handleEditProvider}
        appId={ACTIVE_APP}
        isProxyTakeover={false}
        isCurrent={effectiveEditingProvider?.id === currentProviderId}
      />

      {effectiveUsageProvider && (
        <UsageScriptModal
          key={effectiveUsageProvider.id}
          provider={effectiveUsageProvider}
          appId={ACTIVE_APP}
          isOpen={Boolean(usageProvider)}
          onClose={() => setUsageProvider(null)}
          onSave={(script) => {
            if (usageProvider) {
              void saveUsageScript(usageProvider, script);
            }
          }}
        />
      )}

      <ConfirmDialog
        isOpen={Boolean(confirmProvider)}
        title={t("confirm.deleteProvider")}
        message={t("confirm.deleteProviderMessage", {
          name: confirmProvider?.name ?? "",
        })}
        variant="destructive"
        onConfirm={() => void handleConfirmDelete()}
        onCancel={() => setConfirmProvider(null)}
      />

      <ConfirmDialog
        isOpen={pendingLeave !== null}
        title={t("common.unsavedLeaveTitle")}
        message={t("common.unsavedLeaveMessage")}
        confirmText={t("common.unsavedLeaveConfirm")}
        cancelText={t("common.unsavedLeaveCancel")}
        zIndex="top"
        onConfirm={() => {
          const proceed = pendingLeave;
          setPendingLeave(null);
          discardUnsavedChanges();
          proceed?.();
        }}
        onCancel={() => setPendingLeave(null)}
      />

      <DeepLinkImportDialog />
      <FirstRunNoticeDialog />
      <NewLayoutDialog />
      <WhatsNewNotice />
    </WindowControlsContext.Provider>
  );
}

export default App;
