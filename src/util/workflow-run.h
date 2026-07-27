#pragma once

#include <glib.h>
#include <json-glib/json-glib.h>

G_BEGIN_DECLS

/*
 * Replaces a `gh run watch` or `gh run view` tool summary with a durable
 * workflow record.
 *
 * The record stays a tool message, so it survives transcript reloads and
 * remote viewing without being replayed to the model as conversation.
 */
char     *xd_workflow_run_capture_tool (const char *message,
                                        const char *workdir);

/*
 * Reads a captured workflow record.
 *
 * Both outputs are newly allocated when requested. False means @message is an
 * ordinary tool record.
 */
gboolean  xd_workflow_run_from_tool    (const char *message,
                                        char      **run_id,
                                        char      **url);

/*
 * Summarizes the live job/step activity exposed by GitHub's run API.
 *
 * Raw job logs cannot be downloaded until a job completes. These lines mirror
 * the useful live portion of `gh run watch` without starting another process.
 */
char     *xd_workflow_run_activity     (JsonArray  *jobs,
                                        guint       limit);

G_END_DECLS
