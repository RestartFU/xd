#pragma once

#include <adwaita.h>

G_BEGIN_DECLS

typedef enum
{
  HY_MESSAGE_USER,
  HY_MESSAGE_ASSISTANT,
  HY_MESSAGE_TOOL,
  HY_MESSAGE_ERROR,
} HyMessageKind;

#define HY_TYPE_MESSAGE_ROW (hy_message_row_get_type ())
G_DECLARE_FINAL_TYPE (HyMessageRow, hy_message_row, HY, MESSAGE_ROW, AdwBin)

/*
 * One message in the transcript.
 *
 * Assistant rows are created empty, hold a spinner while the backend is
 * talking, and are given their text by hy_message_row_set_text() once the
 * message is complete.
 */
HyMessageRow *hy_message_row_new        (HyMessageKind  kind,
                                         const char    *text);

void          hy_message_row_append     (HyMessageRow  *self,
                                         const char    *delta);

const char   *hy_message_row_get_text   (HyMessageRow  *self);

/* Replaces what the row shows. */
void          hy_message_row_set_text   (HyMessageRow  *self,
                                         const char    *text);

/* Records what produced the message -- model and effort -- as a tooltip. */
void          hy_message_row_set_source (HyMessageRow  *self,
                                         const char    *source);

/* Shows a spinner while the row is still waiting for its first token. */
void          hy_message_row_set_waiting (HyMessageRow *self,
                                          gboolean      waiting);

HyMessageKind hy_message_kind_from_role (const char *role);
const char   *hy_message_kind_to_role   (HyMessageKind kind);

G_END_DECLS
