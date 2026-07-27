#pragma once

#include <adwaita.h>

G_BEGIN_DECLS

typedef struct _XdRemoteClient XdRemoteClient;

typedef enum
{
  XD_MESSAGE_USER,
  XD_MESSAGE_ASSISTANT,
  XD_MESSAGE_TOOL,
  XD_MESSAGE_ERROR,
} XdMessageKind;

#define XD_TYPE_MESSAGE_ROW (xd_message_row_get_type ())
G_DECLARE_FINAL_TYPE (XdMessageRow, xd_message_row, XD, MESSAGE_ROW, AdwBin)

/*
 * One message in the transcript.
 *
 * Rows represent complete messages. Turn progress belongs to the transcript's
 * working marker, so incomplete assistant text does not allocate blank space.
 */
XdMessageRow *xd_message_row_new         (XdMessageKind  kind,
                                          const char    *text);
void          xd_message_row_make_workflow (XdMessageRow *self,
                                             const char   *run_id,
                                             const char   *url);
void          xd_message_row_make_subagent (XdMessageRow *self);

/* Records what produced the message -- model and effort -- as a tooltip. */
void          xd_message_row_set_source (XdMessageRow  *self,
                                         const char    *source);

/* Lets daemon-local image paths be previewed through their paired client. */
void          xd_message_row_set_remote (XdMessageRow   *self,
                                         XdRemoteClient *remote);

XdMessageKind xd_message_kind_from_role (const char *role);
const char   *xd_message_kind_to_role   (XdMessageKind kind);

G_END_DECLS
