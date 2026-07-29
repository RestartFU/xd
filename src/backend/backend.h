#pragma once

#include <gio/gio.h>
#include <json-glib/json-glib.h>

G_BEGIN_DECLS

/*
 * xd never speaks to an AI API. It drives the CLIs already installed and
 * authenticated on the machine -- claude, codex and opencode -- and reads the
 * JSONL they write to stdout.
 *
 * A backend contributes two things: how to build the command line, and how to
 * turn one line of its output into events. Spawning, reading and cancelling
 * are the same for every backend and live in XdChatSession.
 */

typedef enum
{
  AI_EVENT_SESSION_STARTED,   /* carries the id needed to resume later */
  AI_EVENT_COMMANDS,          /* installed slash commands, without leading / */
  AI_EVENT_TEXT_DELTA,        /* a piece of the reply */
  AI_EVENT_TOOL_USE,          /* the agent reached for a tool */
  AI_EVENT_USAGE,             /* context used and the model's context window */
  AI_EVENT_RESULT,            /* the turn finished; text is the whole reply */
  AI_EVENT_ERROR,
} AiEventType;

typedef struct
{
  AiEventType type;
  const char *session_id;
  const char *text;
  const char *const *commands;
  guint64 context_used;
  guint64 context_window;
  JsonNode *raw;
} AiEvent;

typedef void (*AiEventFunc) (const AiEvent *event,
                             gpointer       user_data);

/*
 * How much rope the assistant gets in the working directory.
 *
 * Ordered least to most permissive; the CLIs spell these differently but the
 * ladder is the same, and xd defaults to the bottom of it.
 */
typedef enum
{
  AI_ACCESS_PLAN,       /* think it through, change nothing */
  AI_ACCESS_READ_ONLY,
  AI_ACCESS_EDIT,       /* may edit files in the working directory */
  AI_ACCESS_FULL,       /* no confirmation for anything */
} AiAccess;

/* How hard the model is asked to think. Every chat names one. */
typedef enum
{
  AI_EFFORT_LOW,
  AI_EFFORT_MEDIUM,
  AI_EFFORT_HIGH,
  AI_EFFORT_XHIGH,
  AI_EFFORT_MAX,
} AiEffort;

const char *ai_effort_to_string   (AiEffort    effort);
AiEffort    ai_effort_from_string (const char *name);
const char *ai_effort_label       (AiEffort    effort);

const char *ai_access_to_string   (AiAccess    access);
AiAccess    ai_access_from_string (const char *name);
const char *ai_access_label       (AiAccess    access);
const char *ai_access_icon_name   (AiAccess    access);

typedef struct
{
  const char *prompt;
  const char *model;              /* NULL: let the CLI choose */
  const char *system_prompt;      /* NULL: no extra instructions */
  const char *resume_session_id;  /* NULL: start a new session */
  const char *workdir;            /* NULL: inherit ours */
  const char *const *folder_ids;  /* outermost to innermost secret scopes */
  AiEffort effort;
  AiAccess access;
} AiRunSpec;

typedef struct _AiParser AiParser;
typedef struct _AiBackend AiBackend;

/* One selectable model. Every chat names one; there is no "let the CLI
 * decide", because a chat that does not say which model answers it cannot show
 * the user which model answered it. */
typedef struct
{
  const char *id;
  const char *display_name;
  guint64 context_window;     /* fallback when the CLI does not report it */
} AiModel;

struct _AiBackend
{
  const char *id;
  const char *display_name;
  const char *program;          /* looked up in PATH */
  const char *icon_name;

  const AiModel *models;
  gsize n_models;
  const char *default_model;    /* what a new chat gets */

  /* Returns a NULL-terminated argv; the caller owns it. */
  GPtrArray *(*build_argv) (const AiBackend *self,
                            const AiRunSpec *spec);

  /* @root is one parsed line of output. */
  void (*parse_object) (AiParser    *parser,
                        JsonObject  *root,
                        AiEventFunc  callback,
                        gpointer     user_data);
};

/*
 * What the CLI would do if xd said nothing.
 *
 * Read from the CLI's own configuration, so the value xd offers is the one
 * the user already had rather than a guess baked in here.
 */
AiEffort                ai_backend_default_effort (const AiBackend *self);

const AiBackend        *ai_backend_lookup (const char *id);
const AiBackend *const *ai_backend_all    (guint      *n_backends);

/* Human-readable name for a model id, falling back to the id itself so an
 * unknown or hand-typed model still shows something meaningful. */
const char             *ai_backend_model_label (const AiBackend *self,
                                                const char      *model_id);
guint64                 ai_backend_context_window (const AiBackend *self,
                                                   const char      *model_id);

/*
 * Per-run parser state.
 *
 * Some backends report the same text twice -- claude streams deltas and then
 * repeats the finished message -- so the parser has to remember what it has
 * already handed out.
 */
/* A tool call whose arguments are still streaming in. */
typedef struct
{
  char *id;
  char *name;
  GString *json;
} AiPendingTool;

struct _AiParser
{
  const AiBackend *backend;
  JsonParser *json;
  gboolean streamed_text;
  char *session_id;  /* backends whose id rides on every event emit it once */
  char *model;       /* model selected for this run */

  /* Block index -> AiPendingTool*. Tool arguments arrive in fragments after
   * the block opens, so the call cannot be described until it closes. */
  GHashTable *pending_tools;

  /* Claude tool-use id -> summary. File-mutating calls are announced before
   * Claude executes them, so their event waits for the matching tool result. */
  GHashTable *deferred_file_tools;

  /* Codex command id -> present. Commands are useful while they are running,
   * so their start event is emitted and their completion is deduplicated. */
  GHashTable *started_commands;
};

AiParser *ai_parser_new       (const AiBackend *backend);
void      ai_parser_free      (AiParser        *self);
void      ai_parser_set_model (AiParser        *self,
                               const char      *model);

/* Malformed lines are logged and skipped: a backend hiccup must not take the
 * conversation down with it. */
void      ai_parser_feed_line (AiParser        *self,
                               const char      *line,
                               AiEventFunc      callback,
                               gpointer         user_data);

G_DEFINE_AUTOPTR_CLEANUP_FUNC (AiParser, ai_parser_free)

/* Helpers shared by the backend implementations. */
const char *ai_json_get_string (JsonObject *object,
                                const char *member);
JsonObject *ai_json_get_object (JsonObject *object,
                                const char *member);

/*
 * "Read" on its own says nothing; "Read src/backend/backend.c" says what is
 * happening. Picks the argument that identifies the work -- the command, the
 * path, the pattern -- and appends it to the tool's name.
 */
char       *ai_tool_summary    (const char *tool_name,
                                JsonObject *input);
gboolean    ai_tool_changes_files (const char *tool_name);

G_END_DECLS
