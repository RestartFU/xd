module Xd
  module Protocol
    # Complete client-to-daemon command vocabulary.
    #
    # Local and remote clients use this same enum and dispatch path. Transport
    # decides how a connection authenticates, never which application logic
    # runs.
    enum Operation
      Invalid
      Pair
      Hello
      Tree
      Messages
      Search
      ImageRead
      ListDir
      FileBrowse
      AgentSecrets
      SetAgentSecrets
      AgentAuth
      AgentAuthStart
      AgentAuthInput
      AgentAuthCancel
      AgentAuthLogout
      AgentClis
      NewFolder
      RenameFolder
      MoveFolder
      TrashFolder
      FolderContext
      SetFolderContext
      FolderSettings
      SetFolderSettings
      NewChat
      RenameChat
      DeleteChat
      Chat
      SetOption
      Send
      Queue
      DropQueue
      EditQueue
      SteerQueue
      Cancel
      DiffRead
      GitState
      GitDraft
      GitAction
      TerminalList
      TerminalOpen
      TerminalInput
      TerminalResize
      TerminalKill
      VoiceModel
      VoiceModelDownload
      VoiceTranscribe
      VoiceCancel
      Ping

      def wire_name : String
        case self
        when Pair               then "pair"
        when Hello              then "hello"
        when Tree               then "tree"
        when Messages           then "messages"
        when Search             then "search"
        when ImageRead          then "image-read"
        when ListDir            then "list-dir"
        when FileBrowse         then "file-browse"
        when AgentSecrets       then "agent-secrets"
        when SetAgentSecrets    then "set-agent-secrets"
        when AgentAuth          then "agent-auth"
        when AgentAuthStart     then "agent-auth-start"
        when AgentAuthInput     then "agent-auth-input"
        when AgentAuthCancel    then "agent-auth-cancel"
        when AgentAuthLogout    then "agent-auth-logout"
        when AgentClis          then "agent-clis"
        when NewFolder          then "new-folder"
        when RenameFolder       then "rename-folder"
        when MoveFolder         then "move-folder"
        when TrashFolder        then "trash-folder"
        when FolderContext      then "folder-context"
        when SetFolderContext   then "set-folder-context"
        when FolderSettings     then "folder-settings"
        when SetFolderSettings  then "set-folder-settings"
        when NewChat            then "new-chat"
        when RenameChat         then "rename-chat"
        when DeleteChat         then "delete-chat"
        when Chat               then "chat"
        when SetOption          then "set-option"
        when Send               then "send"
        when Queue              then "queue"
        when DropQueue          then "drop-queue"
        when EditQueue          then "edit-queue"
        when SteerQueue         then "steer-queue"
        when Cancel             then "cancel"
        when DiffRead           then "diff-read"
        when GitState           then "git-state"
        when GitDraft           then "git-draft"
        when GitAction          then "git-action"
        when TerminalList       then "terminal-list"
        when TerminalOpen       then "terminal-open"
        when TerminalInput      then "terminal-input"
        when TerminalResize     then "terminal-resize"
        when TerminalKill       then "terminal-kill"
        when VoiceModel         then "voice-model"
        when VoiceModelDownload then "voice-model-download"
        when VoiceTranscribe    then "voice-transcribe"
        when VoiceCancel        then "voice-cancel"
        when Ping               then "ping"
        else                         ""
        end
      end

      def self.from_wire?(name : String) : self?
        case name
        when "pair"                 then Pair
        when "hello"                then Hello
        when "tree"                 then Tree
        when "messages"             then Messages
        when "search"               then Search
        when "image-read"           then ImageRead
        when "list-dir"             then ListDir
        when "file-browse"          then FileBrowse
        when "agent-secrets"        then AgentSecrets
        when "set-agent-secrets"    then SetAgentSecrets
        when "agent-auth"           then AgentAuth
        when "agent-auth-start"     then AgentAuthStart
        when "agent-auth-input"     then AgentAuthInput
        when "agent-auth-cancel"    then AgentAuthCancel
        when "agent-auth-logout"    then AgentAuthLogout
        when "agent-clis"           then AgentClis
        when "new-folder"           then NewFolder
        when "rename-folder"        then RenameFolder
        when "move-folder"          then MoveFolder
        when "trash-folder"         then TrashFolder
        when "folder-context"       then FolderContext
        when "set-folder-context"   then SetFolderContext
        when "folder-settings"      then FolderSettings
        when "set-folder-settings"  then SetFolderSettings
        when "new-chat"             then NewChat
        when "rename-chat"          then RenameChat
        when "delete-chat"          then DeleteChat
        when "chat"                 then Chat
        when "set-option"           then SetOption
        when "send"                 then Send
        when "queue"                then Queue
        when "drop-queue"           then DropQueue
        when "edit-queue"           then EditQueue
        when "steer-queue"          then SteerQueue
        when "cancel"               then Cancel
        when "diff-read"            then DiffRead
        when "git-state"            then GitState
        when "git-draft"            then GitDraft
        when "git-action"           then GitAction
        when "terminal-list"        then TerminalList
        when "terminal-open"        then TerminalOpen
        when "terminal-input"       then TerminalInput
        when "terminal-resize"      then TerminalResize
        when "terminal-kill"        then TerminalKill
        when "voice-model"          then VoiceModel
        when "voice-model-download" then VoiceModelDownload
        when "voice-transcribe"     then VoiceTranscribe
        when "voice-cancel"         then VoiceCancel
        when "ping"                 then Ping
        else                             nil
        end
      end

      def authentication_required? : Bool
        self != Pair && self != Hello
      end
    end
  end
end
