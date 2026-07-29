#pragma once

#include "backend.h"

G_BEGIN_DECLS

typedef struct _XdCodexTurn XdCodexTurn;

typedef void (*XdCodexTurnFinishedFunc) (gboolean    success,
                                         const char *message,
                                         gpointer    user_data);

XdCodexTurn *xd_codex_app_server_start (const AiBackend          *backend,
                                        const AiRunSpec          *spec,
                                        GStrv                     environment,
                                        GStrv                     secret_names,
                                        AiEventFunc               event_callback,
                                        XdCodexTurnFinishedFunc   finished_callback,
                                        gpointer                  user_data,
                                        GError                  **error);
void         xd_codex_turn_cancel      (XdCodexTurn              *turn);
void         xd_codex_turn_detach      (XdCodexTurn              *turn);
void         xd_codex_turn_free        (XdCodexTurn              *turn);
void         xd_codex_app_server_shutdown_all (void);

G_END_DECLS
