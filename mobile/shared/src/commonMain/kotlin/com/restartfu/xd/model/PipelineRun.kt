package com.restartfu.xd.model

/** A GitHub Actions run marker captured by the daemon from a tool call. */
public data class PipelineRun(
    val id: String,
    val repository: String,
    val url: String,
    val marker: String,
) {
    public companion object {
        private const val PREFIX = "workflow_run\n"
        private const val URL_PREFIX = "https://github.com/"

        public fun parse(text: String): PipelineRun? {
            if (!text.startsWith(PREFIX)) return null
            val body = text.removePrefix(PREFIX)
            val parts = body.split('\n', limit = 2)
            if (parts.size != 2) return null

            val id = parts[0]
            val url = parts[1]
            if (id.isEmpty() || !id.all { it in '0'..'9' }) return null
            val suffix = "/actions/runs/$id"
            if (!url.startsWith(URL_PREFIX) || !url.endsWith(suffix)) return null

            val repository = url.substring(
                URL_PREFIX.length,
                url.length - suffix.length,
            )
            val components = repository.split('/')
            if (
                components.size != 2 ||
                components.any { component ->
                    component.isEmpty() ||
                        component.any { character ->
                            !character.isLetterOrDigit() &&
                                character != '-' &&
                                character != '_' &&
                                character != '.'
                        }
                }
            ) {
                return null
            }
            return PipelineRun(id, repository, url, text)
        }
    }
}
