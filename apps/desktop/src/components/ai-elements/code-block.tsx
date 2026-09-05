"use client";

import { Button } from "@/components/ui/button";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { cn } from "@/lib/utils";
import {
  CheckIcon,
  ChevronDownIcon,
  ChevronUpIcon,
  CopyIcon,
  DownloadIcon,
  FileCode2Icon,
} from "lucide-react";
import type { ComponentProps, CSSProperties, HTMLAttributes } from "react";
import {
  createContext,
  memo,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import type {
  HighlighterCore,
  ThemedToken,
} from "shiki/types";

// Shiki core, regex engine and themes are all loaded lazily on the first code
// block render, so the highlighter never enters the cold-start entry. A raw
// plaintext render is shown meanwhile (see createRawTokens).

// Shiki uses bitflags for font styles: 1=italic, 2=bold, 4=underline
// oxlint-disable-next-line eslint(no-bitwise)
const isItalic = (fontStyle: number | undefined) => fontStyle && fontStyle & 1;
// oxlint-disable-next-line eslint(no-bitwise)
const isBold = (fontStyle: number | undefined) => fontStyle && fontStyle & 2;
const isUnderline = (fontStyle: number | undefined) =>
  // oxlint-disable-next-line eslint(no-bitwise)
  fontStyle && fontStyle & 4;

// Transform tokens to include pre-computed keys to avoid noArrayIndexKey lint
interface KeyedToken {
  token: ThemedToken;
  key: string;
}
interface KeyedLine {
  tokens: KeyedToken[];
  key: string;
}

const addKeysToTokens = (lines: ThemedToken[][]): KeyedLine[] =>
  lines.map((line, lineIdx) => ({
    key: `line-${lineIdx}`,
    tokens: line.map((token, tokenIdx) => ({
      key: `line-${lineIdx}-${tokenIdx}`,
      token,
    })),
  }));

// Token rendering component
const TokenSpan = ({ token }: { token: ThemedToken }) => (
  <span
    className="dark:!bg-[var(--shiki-dark-bg)] dark:!text-[var(--shiki-dark)]"
    style={
      {
        backgroundColor: token.bgColor,
        color: token.color,
        fontStyle: isItalic(token.fontStyle) ? "italic" : undefined,
        fontWeight: isBold(token.fontStyle) ? "bold" : undefined,
        textDecoration: isUnderline(token.fontStyle) ? "underline" : undefined,
        ...token.htmlStyle,
      } as CSSProperties
    }
  >
    {token.content}
  </span>
);

// Line number styles using CSS counters
const LINE_NUMBER_CLASSES = cn(
  "block",
  "before:content-[counter(line)]",
  "before:inline-block",
  "before:[counter-increment:line]",
  "before:w-8",
  "before:mr-4",
  "before:text-right",
  "before:text-muted-foreground/50",
  "before:font-mono",
  "before:select-none"
);

// Line rendering component
const LineSpan = ({
  keyedLine,
  showLineNumbers,
}: {
  keyedLine: KeyedLine;
  showLineNumbers: boolean;
}) => (
  <span className={showLineNumbers ? LINE_NUMBER_CLASSES : "block"}>
    {keyedLine.tokens.length === 0
      ? "\n"
      : keyedLine.tokens.map(({ token, key }) => (
          <TokenSpan key={key} token={token} />
        ))}
  </span>
);

// Types
type CodeBlockProps = HTMLAttributes<HTMLDivElement> & {
  code: string;
  language: string;
  showLineNumbers?: boolean;
};

interface TokenizedCode {
  tokens: ThemedToken[][];
  fg: string;
  bg: string;
}

interface CodeBlockContextType {
  code: string;
}

// Context
const CodeBlockContext = createContext<CodeBlockContextType>({
  code: "",
});

const languageLoaders = {
  css: () => import("@shikijs/langs/css"),
  diff: () => import("@shikijs/langs/diff"),
  html: () => import("@shikijs/langs/html"),
  javascript: () => import("@shikijs/langs/javascript"),
  json: () => import("@shikijs/langs/json"),
  jsx: () => import("@shikijs/langs/jsx"),
  markdown: () => import("@shikijs/langs/markdown"),
  powershell: () => import("@shikijs/langs/powershell"),
  python: () => import("@shikijs/langs/python"),
  rust: () => import("@shikijs/langs/rust"),
  shellscript: () => import("@shikijs/langs/shellscript"),
  sql: () => import("@shikijs/langs/sql"),
  tsx: () => import("@shikijs/langs/tsx"),
  typescript: () => import("@shikijs/langs/typescript"),
  yaml: () => import("@shikijs/langs/yaml"),
} as const;

type SupportedLanguage = keyof typeof languageLoaders | "plaintext";

const languageAliases: Record<string, SupportedLanguage> = {
  js: "javascript", md: "markdown", ps1: "powershell", py: "python",
  rs: "rust", sh: "shellscript", shell: "shellscript", text: "plaintext",
  ts: "typescript", yml: "yaml",
};

const normalizeLanguage = (language: string): SupportedLanguage => {
  const normalized = language.trim().toLowerCase();
  if (!normalized) return "plaintext";
  if (normalized in languageAliases) return languageAliases[normalized];
  return normalized in languageLoaders ? normalized as keyof typeof languageLoaders : "plaintext";
};

let highlighterPromise: Promise<HighlighterCore> | null = null;

const loadHighlighter = () => {
  if (!highlighterPromise) {
    highlighterPromise = Promise.all([
      import("shiki/core"),
      import("shiki/engine/javascript"),
      import("@shikijs/themes/github-light"),
      import("@shikijs/themes/github-dark"),
    ]).then(([core, engine, light, dark]) => core.createHighlighterCore({
      engine: engine.createJavaScriptRegexEngine(),
      langs: [],
      themes: [light.default, dark.default],
    }));
  }
  return highlighterPromise;
};

const languageLoadCache = new Map<string, Promise<void>>();

// Token cache
const tokensCache = new Map<string, TokenizedCode>();

// Subscribers for async token updates
const subscribers = new Map<string, Set<(result: TokenizedCode) => void>>();

const getTokensCacheKey = (code: string, language: string) => {
  const start = code.slice(0, 100);
  const end = code.length > 100 ? code.slice(-100) : "";
  return `${language}:${code.length}:${start}:${end}`;
};

const getHighlighter = async (language: string) => {
  const normalized = normalizeLanguage(language);
  const highlighter = await loadHighlighter();
  if (normalized === "plaintext" || highlighter.getLoadedLanguages().includes(normalized)) {
    return { highlighter, language: normalized };
  }

  let loading = languageLoadCache.get(normalized);
  if (!loading) {
    loading = languageLoaders[normalized]().then((module) => {
      highlighter.loadLanguage(...module.default);
    });
    languageLoadCache.set(normalized, loading);
  }
  await loading;
  return { highlighter, language: normalized };
};

// Create raw tokens for immediate display while highlighting loads
const createRawTokens = (code: string): TokenizedCode => ({
  bg: "transparent",
  fg: "inherit",
  tokens: code.split("\n").map((line) =>
    line === ""
      ? []
      : [
          {
            color: "inherit",
            content: line,
          } as ThemedToken,
        ]
  ),
});

// Synchronous highlight with callback for async results
export const highlightCode = (
  code: string,
  language: string,
  // oxlint-disable-next-line eslint-plugin-promise(prefer-await-to-callbacks)
  callback?: (result: TokenizedCode) => void
): TokenizedCode | null => {
  const tokensCacheKey = getTokensCacheKey(code, language);

  // Return cached result if available
  const cached = tokensCache.get(tokensCacheKey);
  if (cached) {
    return cached;
  }

  // Subscribe callback if provided
  if (callback) {
    if (!subscribers.has(tokensCacheKey)) {
      subscribers.set(tokensCacheKey, new Set());
    }
    subscribers.get(tokensCacheKey)?.add(callback);
  }

  // Start highlighting in background - fire-and-forget async pattern
  getHighlighter(language)
    // oxlint-disable-next-line eslint-plugin-promise(prefer-await-to-then)
    .then(({ highlighter, language: langToUse }) => {
      const result = highlighter.codeToTokens(code, {
        lang: langToUse,
        themes: {
          dark: "github-dark",
          light: "github-light",
        },
      });

      const tokenized: TokenizedCode = {
        bg: result.bg ?? "transparent",
        fg: result.fg ?? "inherit",
        tokens: result.tokens,
      };

      // Cache the result
      tokensCache.set(tokensCacheKey, tokenized);

      // Notify all subscribers
      const subs = subscribers.get(tokensCacheKey);
      if (subs) {
        for (const sub of subs) {
          sub(tokenized);
        }
        subscribers.delete(tokensCacheKey);
      }
    })
    // oxlint-disable-next-line eslint-plugin-promise(prefer-await-to-then), eslint-plugin-promise(prefer-await-to-callbacks)
    .catch((error) => {
      console.error("Failed to highlight code:", error);
      subscribers.delete(tokensCacheKey);
    });

  return null;
};

const CodeBlockBody = memo(
  ({
    tokenized,
    showLineNumbers,
    className,
  }: {
    tokenized: TokenizedCode;
    showLineNumbers: boolean;
    className?: string;
  }) => {
    const preStyle = useMemo(
      () => ({
        backgroundColor: tokenized.bg,
        color: tokenized.fg,
      }),
      [tokenized.bg, tokenized.fg]
    );

    const keyedLines = useMemo(
      () => addKeysToTokens(tokenized.tokens),
      [tokenized.tokens]
    );

    return (
      <pre
        className={cn(
          "dark:!bg-[var(--shiki-dark-bg)] dark:!text-[var(--shiki-dark)] m-0 p-4 text-sm",
          className
        )}
        style={preStyle}
      >
        <code
          className={cn(
            "font-mono text-sm",
            showLineNumbers && "[counter-increment:line_0] [counter-reset:line]"
          )}
        >
          {keyedLines.map((keyedLine) => (
            <LineSpan
              key={keyedLine.key}
              keyedLine={keyedLine}
              showLineNumbers={showLineNumbers}
            />
          ))}
        </code>
      </pre>
    );
  },
  (prevProps, nextProps) =>
    prevProps.tokenized === nextProps.tokenized &&
    prevProps.showLineNumbers === nextProps.showLineNumbers &&
    prevProps.className === nextProps.className
);

CodeBlockBody.displayName = "CodeBlockBody";

export const CodeBlockContainer = ({
  className,
  language,
  style,
  ...props
}: HTMLAttributes<HTMLDivElement> & { language: string }) => (
  <div
    className={cn(
      "group relative w-full overflow-hidden rounded-md border bg-background text-foreground",
      className
    )}
    data-language={language}
    style={{
      containIntrinsicSize: "auto 200px",
      contentVisibility: "auto",
      ...style,
    }}
    {...props}
  />
);

export const CodeBlockHeader = ({
  children,
  className,
  ...props
}: HTMLAttributes<HTMLDivElement>) => (
  <div
    className={cn(
      "flex items-center justify-between border-b bg-muted/80 px-3 py-2 text-muted-foreground text-xs",
      className
    )}
    {...props}
  >
    {children}
  </div>
);

export const CodeBlockTitle = ({
  children,
  className,
  ...props
}: HTMLAttributes<HTMLDivElement>) => (
  <div className={cn("flex items-center gap-2", className)} {...props}>
    {children}
  </div>
);

export const CodeBlockFilename = ({
  children,
  className,
  ...props
}: HTMLAttributes<HTMLSpanElement>) => (
  <span className={cn("font-mono", className)} {...props}>
    {children}
  </span>
);

export const CodeBlockActions = ({
  children,
  className,
  ...props
}: HTMLAttributes<HTMLDivElement>) => (
  <div
    className={cn("-my-1 -mr-1 flex items-center gap-2", className)}
    {...props}
  >
    {children}
  </div>
);

export const CodeBlockContent = ({
  code,
  language,
  showLineNumbers = false,
}: {
  code: string;
  language: string;
  showLineNumbers?: boolean;
}) => {
  // Memoized raw tokens for immediate display
  const rawTokens = useMemo(() => createRawTokens(code), [code]);

  // Synchronous cache lookup — avoids setState in effect for cached results
  const syncTokens = useMemo(
    () => highlightCode(code, language) ?? rawTokens,
    [code, language, rawTokens]
  );

  // Async highlighting result (populated after shiki loads)
  const [asyncTokens, setAsyncTokens] = useState<TokenizedCode | null>(null);
  const asyncKeyRef = useRef({ code, language });

  // Invalidate stale async tokens synchronously during render
  if (
    asyncKeyRef.current.code !== code ||
    asyncKeyRef.current.language !== language
  ) {
    asyncKeyRef.current = { code, language };
    setAsyncTokens(null);
  }

  useEffect(() => {
    let cancelled = false;

    highlightCode(code, language, (result) => {
      if (!cancelled) {
        setAsyncTokens(result);
      }
    });

    return () => {
      cancelled = true;
    };
  }, [code, language]);

  const tokenized = asyncTokens ?? syncTokens;

  return (
    <div className="relative overflow-auto">
      <CodeBlockBody showLineNumbers={showLineNumbers} tokenized={tokenized} />
    </div>
  );
};

export const CodeBlock = ({
  code,
  language,
  showLineNumbers = false,
  className,
  children,
  ...props
}: CodeBlockProps) => {
  const contextValue = useMemo(() => ({ code }), [code]);

  return (
    <CodeBlockContext.Provider value={contextValue}>
      <CodeBlockContainer className={className} language={language} {...props}>
        {children}
        <CodeBlockContent
          code={code}
          language={language}
          showLineNumbers={showLineNumbers}
        />
      </CodeBlockContainer>
    </CodeBlockContext.Provider>
  );
};

export type CollapsibleCodeBlockProps = CodeBlockProps & {
  filename?: string;
  previewLines?: number;
  isIncomplete?: boolean;
};

export const CollapsibleCodeBlock = ({
  code,
  language,
  filename,
  previewLines = 6,
  showLineNumbers = true,
  isIncomplete = false,
  className,
  ...props
}: CollapsibleCodeBlockProps) => {
  const [isOpen, setIsOpen] = useState(false);
  const lines = useMemo(() => code.split("\n"), [code]);
  const preview = useMemo(
    () => lines.slice(0, previewLines).join("\n"),
    [lines, previewLines]
  );
  const contextValue = useMemo(() => ({ code }), [code]);
  const canExpand = lines.length > previewLines;
  const downloadCode = useCallback(() => {
    if (typeof document === "undefined") return;
    const extensionByLanguage: Record<string, string> = {
      bash: "sh",
      bat: "bat",
      cmd: "cmd",
      console: "txt",
      javascript: "js",
      jsx: "jsx",
      markdown: "md",
      output: "txt",
      plaintext: "txt",
      powershell: "ps1",
      python: "py",
      shell: "sh",
      shellscript: "sh",
      terminal: "txt",
      typescript: "ts",
    };
    const baseName = filename?.trim() || `output.${extensionByLanguage[language] || language || "txt"}`;
    const url = URL.createObjectURL(new Blob([code], { type: "text/plain;charset=utf-8" }));
    const link = document.createElement("a");
    link.href = url;
    link.download = baseName;
    document.body.append(link);
    link.click();
    link.remove();
    window.setTimeout(() => URL.revokeObjectURL(url), 0);
  }, [code, filename, language]);

  return (
    <CodeBlockContext.Provider value={contextValue}>
      <Collapsible
        className="fox-collapsible-code"
        onOpenChange={setIsOpen}
        open={isOpen}
      >
        <CodeBlockContainer
          className={cn("fox-collapsible-code-block", className)}
          language={language}
          {...props}
        >
          <CodeBlockHeader className="fox-collapsible-code-header">
            <CodeBlockTitle className="fox-collapsible-code-title">
              <FileCode2Icon size={14} />
              <CodeBlockFilename>{filename || language}</CodeBlockFilename>
              <span>{isIncomplete ? "生成中" : `${lines.length} 行`}</span>
            </CodeBlockTitle>
            <CodeBlockActions className="fox-collapsible-code-actions">
              <CodeBlockCopyButton title="复制代码" />
              <Button aria-label="下载代码" onClick={downloadCode} size="icon" title="下载代码" type="button" variant="ghost">
                <DownloadIcon size={14} />
              </Button>
              {canExpand && <CollapsibleTrigger asChild>
                <Button size="sm" type="button" variant="ghost">
                  {isOpen ? (
                    <><ChevronUpIcon size={14} />收起代码</>
                  ) : (
                    <><ChevronDownIcon size={14} />展开代码</>
                  )}
                </Button>
              </CollapsibleTrigger>}
            </CodeBlockActions>
          </CodeBlockHeader>
          {!isOpen && (
            <div className="fox-collapsible-code-preview">
              <CodeBlockContent
                code={preview}
                language={language}
                showLineNumbers={showLineNumbers}
              />
              {lines.length > previewLines && <div className="fox-code-preview-fade" />}
            </div>
          )}
          <CollapsibleContent className="fox-collapsible-code-content">
            <CodeBlockContent
              code={code}
              language={language}
              showLineNumbers={showLineNumbers}
            />
          </CollapsibleContent>
        </CodeBlockContainer>
      </Collapsible>
    </CodeBlockContext.Provider>
  );
};

export type CodeBlockCopyButtonProps = ComponentProps<typeof Button> & {
  onCopy?: () => void;
  onError?: (error: Error) => void;
  timeout?: number;
};

export const CodeBlockCopyButton = ({
  onCopy,
  onError,
  timeout = 2000,
  children,
  className,
  ...props
}: CodeBlockCopyButtonProps) => {
  const [isCopied, setIsCopied] = useState(false);
  const timeoutRef = useRef<number>(0);
  const { code } = useContext(CodeBlockContext);

  const copyToClipboard = useCallback(async () => {
    if (typeof window === "undefined") return;

    try {
      if (!isCopied) {
        let copiedWithClipboardApi = false;
        if (navigator?.clipboard?.writeText) {
          try {
            await navigator.clipboard.writeText(code);
            copiedWithClipboardApi = true;
          } catch {
            copiedWithClipboardApi = false;
          }
        }
        if (!copiedWithClipboardApi) {
          const textarea = document.createElement("textarea");
          textarea.value = code;
          textarea.style.position = "fixed";
          textarea.style.opacity = "0";
          document.body.append(textarea);
          textarea.select();
          const copied = document.execCommand("copy");
          textarea.remove();
          if (!copied) throw new Error("Clipboard API not available");
        }
        setIsCopied(true);
        onCopy?.();
        timeoutRef.current = window.setTimeout(
          () => setIsCopied(false),
          timeout
        );
      }
    } catch (error) {
      onError?.(error as Error);
    }
  }, [code, onCopy, onError, timeout, isCopied]);

  useEffect(
    () => () => {
      window.clearTimeout(timeoutRef.current);
    },
    []
  );

  const Icon = isCopied ? CheckIcon : CopyIcon;

  return (
    <Button
      className={cn("shrink-0", className)}
      onClick={copyToClipboard}
      size="icon"
      variant="ghost"
      {...props}
    >
      {children ?? <Icon size={14} />}
    </Button>
  );
};

export type CodeBlockLanguageSelectorProps = ComponentProps<typeof Select>;

export const CodeBlockLanguageSelector = (
  props: CodeBlockLanguageSelectorProps
) => <Select {...props} />;

export type CodeBlockLanguageSelectorTriggerProps = ComponentProps<
  typeof SelectTrigger
>;

export const CodeBlockLanguageSelectorTrigger = ({
  className,
  ...props
}: CodeBlockLanguageSelectorTriggerProps) => (
  <SelectTrigger
    className={cn(
      "h-7 border-none bg-transparent px-2 text-xs shadow-none",
      className
    )}
    size="sm"
    {...props}
  />
);

export type CodeBlockLanguageSelectorValueProps = ComponentProps<
  typeof SelectValue
>;

export const CodeBlockLanguageSelectorValue = (
  props: CodeBlockLanguageSelectorValueProps
) => <SelectValue {...props} />;

export type CodeBlockLanguageSelectorContentProps = ComponentProps<
  typeof SelectContent
>;

export const CodeBlockLanguageSelectorContent = ({
  align = "end",
  ...props
}: CodeBlockLanguageSelectorContentProps) => (
  <SelectContent align={align} {...props} />
);

export type CodeBlockLanguageSelectorItemProps = ComponentProps<
  typeof SelectItem
>;

export const CodeBlockLanguageSelectorItem = (
  props: CodeBlockLanguageSelectorItemProps
) => <SelectItem {...props} />;
