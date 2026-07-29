#pragma once

#include "backend.h"

G_BEGIN_DECLS

/* Instructions carried by thread/start or thread/resume, never user input. */
char       *xd_codex_developer_instructions (const AiRunSpec *spec);

/* App-server uses camelCase policy names inside turn/start. */
const char *xd_codex_sandbox_policy_type    (AiAccess         access);

G_END_DECLS
