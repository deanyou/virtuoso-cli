;RB = RAMIC Bridge
;RBD = RB Daemon
;RBM = RB Monitor

; Daemon binary path: resolved at load-time, never hardcoded.
; sh() returns t/nil (not stdout), so `which` cannot be used for path detection.
; Priority: (1) RB_DAEMON_PATH env var, (2) ~/.cargo/bin/virtuoso-daemon.
; Re-resolved on reload if previously empty or the file no longer exists.
when(!boundp('RBDPath) || RBDPath == "" || RBDPath == nil || !isFile(RBDPath)
    RBDPath = let((fromEnv cargoPath)
        fromEnv = getShellEnvVar("RB_DAEMON_PATH")
        if(fromEnv then
            fromEnv
        else
            cargoPath = strcat(getShellEnvVar("HOME") "/.cargo/bin/virtuoso-daemon")
            if(isFile(cargoPath) then
                cargoPath
            else
                ""
            )
        )
    )
)
when(RBDPath == "" || RBDPath == nil
    printf("[RAMIC Bridge] ERROR: virtuoso-daemon not found.\n")
    printf("[RAMIC Bridge] Set RB_DAEMON_PATH or run: cargo install --path <repo> --bin virtuoso-daemon --features daemon\n")
)
; RBPython is no longer used (daemon is a standalone binary), kept for compatibility
unless(boundp('RBPython) RBPython = "")
; All state vars preserved across reloads — only initialized if not yet bound.
; RBPort must NOT be reset on reload: the running daemon's actual port would be lost.
unless(boundp('RBPort)      RBPort      = 0)   ; 0 = OS assigns; updated by RBIpcErrHandler
unless(boundp('RBLocal)     RBLocal     = nil)
unless(boundp('RBEcho)      RBEcho      = nil)
unless(boundp('RBDLog)      RBDLog      = nil)

; IPC process handle — must survive reload so we don't spawn a second daemon.
unless(boundp('RBIpc) RBIpc = 'unbound)

; Session management — persist across reloads so session_id stays stable
unless(boundp('RBSessionId)   RBSessionId   = "")
unless(boundp('RBSessionSeq)  RBSessionSeq  = 0)

procedure(RBSendCallback(msg)
  ; Write result to a temp file pair instead of ipcWriteProcess — works around
  ; the IC23.1/RHEL8 bug where ipcWriteProcess stops firing after the first call.
  ; Daemon polls for the .done marker, reads data file, then deletes both.
  let((cbPort dataFile doneFile port)
    cbPort = RBPort + 1
    dataFile = sprintf(nil "/tmp/.ramic_cb_%d" cbPort)
    doneFile = sprintf(nil "/tmp/.ramic_cb_%d.done" cbPort)
    port = outfile(dataFile "w")
    when(port fprintf(port "%s" msg) close(port))
    port = outfile(doneFile "w")
    when(port close(port))
  )
)

procedure(RBIpcDataHandler(ipcId data)
  ; Handler for successful data received from the daemon.
  ; evalstring() bypasses CIW's interactive loop — printf output is line-buffered
  ; and won't appear until a "\n" is received or hiFlush() is called explicitly.
  ; We call hiFlush() after every evalstring to force CIW UI refresh.
  let((result resultStr)
    when(RBEcho printf("[RAMIC Bridge (%L)] receive:%L\n" ipcId data))
    if(errset(result=evalstring(data)) then
        resultStr = sprintf(nil "%c%L%c" intToChar(2) result intToChar(30))
        when(RBEcho printf("[RAMIC Bridge (%L)] return:%L\n" ipcId result))
    else
        resultStr = sprintf(nil "%c%L%c" intToChar(21) errset.errset intToChar(30))
    )
    RBSendCallback(resultStr)
    hiFlush()  ; force CIW UI refresh — evalstring is line-buffered unlike interactive CIW
  )
)

procedure(RBIpcErrHandler(ipcId data)
    ; Parse "PORT:XXXXX" line emitted by daemon on startup to capture actual bound port.
    ; The daemon prints this to stderr before accepting connections.
    rexCompile("PORT:\\([0-9][0-9]*\\)")
    when(rexExecute(data)
        let((actualPort)
            actualPort = atoi(rexSubstitute("\\1"))
            when(actualPort > 0
                RBPort = actualPort
                RBWriteSession(RBSessionId RBPort)
                printf("[RAMIC Bridge] Session '%s' on port %d\n" RBSessionId RBPort)
            )
        )
    )
)

procedure(RBIpcFinishHandler(ipcId data)
  ; Handler called when the Python daemon process exits
  let((exitStatus)
	exitStatus = ipcGetExitStatus(ipcId)
    printf("[RAMIC Bridge (%L)] exit at (%s) with state = %L\n" ipcId getCurrentTime() exitStatus)
  )
)

procedure(RBStart()
    ; Start the RAMIC Bridge daemon process.
    ; Passes port=0 so the OS assigns a free port automatically — supports multiple
    ; Virtuoso instances on the same host without port conflicts.
    ; The daemon prints "PORT:XXXXX" to stderr; RBIpcErrHandler captures it and
    ; writes the session registration file.
	if(boundp('RBIpc) && ipcIsAliveProcess(RBIpc) then
		printf("[RAMIC Bridge (%L)] is already running\n", RBIpc)
	else
		prog((host logpath hostname username)
			; Determine host binding based on RBLocal setting
			if(RBLocal then
				host = "127.0.0.1"  ; Local-only connections
			else
				host = "0.0.0.0"   ; Accept connections from any IP
			)
			; Set log path if logging is enabled
			if(RBDLog then
				logpath = "/tmp/RB.log"
			else
				logpath = ""
			)
			; Generate stable session_id: hostname-username-N (increments per RBStart)
			hostname = getShellEnvVar("HOSTNAME")
			unless(hostname hostname = "localhost")
			hostname = car(parseString(hostname "."))  ; strip domain
			username = getShellEnvVar("USER")
			unless(username username = "user")
			RBSessionSeq = RBSessionSeq + 1
			RBSessionId = sprintf(nil "%s-%s-%d" hostname username RBSessionSeq)
			; Start the daemon with port=0 (OS assigns free port).
			; RBIpcErrHandler will parse PORT:N from stderr and write session file.
			RBIpc = ipcBeginProcess(sprintf(nil "%s %s 0" RBDPath host) "" 'RBIpcDataHandler 'RBIpcErrHandler 'RBIpcFinishHandler logpath)
		)
		printf("[RAMIC Bridge (%L)] start at (%s)\n" RBIpc getCurrentTime())
	)
)

procedure(RBStop()
    ; Stop the RAMIC Bridge daemon process and remove session registration file.
	if(boundp('RBIpc) && ipcIsAliveProcess(RBIpc) then
		ipcKillProcess(RBIpc)
		RBDeleteSession(RBSessionId)
	else
		printf("[RAMIC Bridge] is already down\n" )
	)
)

procedure(RBStopAll()
    ; Emergency function to kill all RAMIC Bridge daemon processes
    ; 1. Try graceful SIGTERM first, then force SIGKILL after 2s
    ; 2. Also kill any process holding the configured RBPort
    ; 3. Reset Skill-side IPC state
    ; 4. Remove this session's registration file
    sh(sprintf(nil "pgrep -f 'virtuoso-daemon' | xargs -r kill 2>/dev/null; sleep 2; pgrep -f 'virtuoso-daemon' | xargs -r kill -9 2>/dev/null; fuser -k %d/tcp 2>/dev/null" RBPort))
    RBDeleteSession(RBSessionId)
    RBIpc = 'unbound
    printf("[RAMIC Bridge] RBStopAll: all daemon processes killed, port %d released\n" RBPort)
)

procedure(RBWriteSession(sessionId port)
    ; Write session registration file using sh() to avoid SKILL fprintf buffering issues.
    ; File: ~/.cache/virtuoso_bridge/sessions/<session_id>.json
    let((dir filename hostname username)
        when(sessionId != ""
            hostname = getShellEnvVar("HOSTNAME")
            unless(hostname hostname = "localhost")
            username = getShellEnvVar("USER")
            unless(username username = "user")
            dir = strcat(getShellEnvVar("HOME") "/.cache/virtuoso_bridge/sessions")
            filename = strcat(dir "/" sessionId ".json")
            ; Use printf to write JSON (avoids sh() output-capture limitation)
            sh(sprintf(nil "mkdir -p \"%s\"" dir))
            sh(sprintf(nil "printf '{\"id\":\"%s\",\"port\":%d,\"pid\":0,\"host\":\"%s\",\"user\":\"%s\",\"created\":\"%s\"}' > \"%s\""
                sessionId port hostname username getCurrentTime() filename))
        )
    )
)

procedure(RBDeleteSession(sessionId)
    ; Remove session registration file on bridge stop.
    let((filename)
        when(sessionId != ""
            filename = strcat(getShellEnvVar("HOME") "/.cache/virtuoso_bridge/sessions/" sessionId ".json")
            when(isFile(filename)
                deleteFile(filename)
            )
        )
    )
)

; ============================================================================
; GUI Components for the RAMIC Bridge Monitor (once per session)
; ============================================================================
; Re-running hiCreateAppForm / hiInsertBannerMenu on reload causes form/menu
; conflicts; build GUI and banner menu only the first time.
unless(boundp('RBMonInstalled)
progn(
; Status label showing current bridge state
RBMState = hiCreateLabel(
	?name 		'RBMState
	?labelText 	"RAMIC Bridge State: ?"
	?justification CDS_JUSTIFY_LEFT
)

; Refresh button to update status display
RBMBtnRefresh = hiCreateButton(
	?name		'RBMBtnRefresh
	?buttonText	"Refresh"
	?callback	"RBMRefresh()"
)

; Start bridge button
RBMBtnStart = hiCreateButton(
	?name		'RBMBtnStart
	?buttonText	"Start"
	?callback	"RBStart() RBMRefresh()"
)

; Stop bridge button
RBMBtnStop = hiCreateButton(
	?name		'RBMBtnStop
	?buttonText	"Stop"
	?callback	"RBStop() RBMRefresh()"
)

; Port number input field
RBMIntPort = hiCreateIntField(
	?name		'RBMIntPort
	?prompt		"Port"
	?value		RBPort
	?defValue	RBPort
	?callback	nil
)

; Local-only connection checkbox
RBMBolLocal = hiCreateBooleanButton(
	?name		'RBMBolLocal
	?buttonText	"Local connect only"
	?value		RBLocal
	?defValue	RBLocal
	?callback	nil
)

; Echo bridging messages checkbox
RBMBolEcho = hiCreateBooleanButton(
	?name		'RBMBolEcho
	?buttonText	"Echo bridging string"
	?value		RBEcho
	?defValue	RBEcho
	?callback	nil
)

; Enable daemon logging checkbox
RBMBolLog = hiCreateBooleanButton(
	?name		'RBMBolLog
	?buttonText	"Daemon log (/tmp/RB.log)"
	?value		RBDLog
	?defValue	RBDLog
	?callback	nil
)

; Emergency kill all daemons button
RBMBtnStopAll = hiCreateButton(
	?name		'RBMBtnStopAll
	?buttonText	"KILL ALL DAEMON (USE AS BACKUP ONLY)"
	?callback	"RBStopAll() RBMRefresh()"
)

; Create the main monitor form
hiCreateAppForm(
	?name 'RBMonitor
	?formTitle "RAMIC Bridge Monitor"
	?fields list(
		list(RBMState		20:10	300:30)    ; Status label
		list(RBMBtnRefresh 	20:50 	100:30)    ; Refresh button
		list(RBMBtnStart  	150:50 	100:30)    ; Start button
		list(RBMBtnStop  	280:50 	100:30)    ; Stop button
		list(RBMIntPort		20:90	170:30 	30)  ; Port input
		list(RBMBolLocal	20:130	170:30)     ; Local-only checkbox
		list(RBMBolEcho		20:170	170:30)     ; Echo checkbox
		list(RBMBolLog		20:210	170:30)     ; Log checkbox
		list(RBMBtnStopAll 	20:250 	360:30)     ; Emergency stop button
	)
	?buttonLayout	'OKCancelApply
	?help			""
	?initialSize		400:320
	?minSize			400:320
	?maxSize			400:320
	?mapCB			"RBMRefresh()"
	?callback 		"RBMApply()"
)

; ============================================================================
; Menu Integration
; ============================================================================

; Create RAMIC menu in Virtuoso's banner
ramicMenu = hiCreatePulldownMenu(
	'ramicMenu
	"RAMIC"
	list(
		hiCreateMenuItem(
			?name 'RAMIC_Bridge
			?itemText "RAMIC Bridge..."
			?callback "hiDisplayForm(RBMonitor)"
		)
		hiCreateMenuItem(
			?name 'RAMIC_PrintLines
			?itemText "Print Empty Lines"
			?callback "RBPrintEmptyLines()"
		)
	)
)

; Insert the RAMIC menu into Virtuoso's main window
hiInsertBannerMenu(window(1) ramicMenu 3)
RBMonInstalled = t
)
) ; end unless(boundp 'RBMonInstalled) + progn

procedure(RBMRefresh()
    ; Refresh the monitor display with current status and settings
	when(boundp('RBMonInstalled)
	if(boundp('RBIpc) && ipcIsAliveProcess(RBIpc) then
		RBMonitor->RBMState->value = sprintf(nil "RAMIC Bridge State: Running | Session: %s | Port: %d" RBSessionId RBPort)
	else
		RBMonitor->RBMState->value = "RAMIC Bridge State: Down"
	)
	RBMonitor->RBMIntPort->value = RBPort
	RBMonitor->RBMBolLocal->value = RBLocal
	RBMonitor->RBMBolEcho->value = RBEcho
	RBMonitor->RBMBolLog->value = RBDLog
	)
)

procedure(RBMApply()
    ; Apply changes from the monitor form to the configuration
	when(boundp('RBMonInstalled)
	RBEcho = RBMonitor->RBMBolEcho->value
	prog((refresh)
	unless(RBMonitor->RBMBolLocal->value == RBLocal
		RBLocal = RBMonitor->RBMBolLocal->value
		refresh = t
	)
	unless(RBMonitor->RBMIntPort->value == RBPort
		RBPort = RBMonitor->RBMIntPort->value
		refresh = t
	)
	unless(RBMonitor->RBMBolLog->value == RBDLog
		RBDLog = RBMonitor->RBMBolLog->value
		refresh = t
	)
	when(refresh
		RBStop()
		RBStart()
	)
	)
	)
)

procedure(RBPrintEmptyLines()
	printf("\n\n\n\n\n\n\n\n\n\n\n\n")
)

; ============================================================================
; virtuoso_cli() — convenience wrapper: start bridge + print connection info
; ============================================================================
; Call this in .cdsinit or CIW instead of calling RBStart() directly.
; Prints session ID and the vcli command to use in the terminal.

procedure(vcli()
    ; Start bridge (no-op if already running)
    RBStart()
    ; Print connection summary
    if(boundp('RBIpc) && ipcIsAliveProcess(RBIpc) then
        printf("\n")
        printf("┌─────────────────────────────────────────┐\n")
        printf("│  vcli (Virtuoso CLI Bridge) — Ready     │\n")
        printf("├─────────────────────────────────────────┤\n")
        printf("│  Session : %-29s │\n" RBSessionId)
        if(RBPort > 0 then
            printf("│  Port    : %-29d │\n" RBPort)
        else
            printf("│  Port    : (assigning...)               │\n")
        )
        printf("├─────────────────────────────────────────┤\n")
        printf("│  Terminal: vcli skill exec 'version()'  │\n")
        printf("│  Sessions: vcli session list            │\n")
        printf("└─────────────────────────────────────────┘\n")
        printf("\n")
        ; Session file is written automatically by RBIpcErrHandler when PORT: arrives.
        ; Do NOT call RBWriteSession here — it would race with the async handler.
    else
        printf("[vcli] Bridge failed to start. Check RBDPath=%s\n" RBDPath)
    )
)

; Auto-start the bridge when this file is loaded (no second daemon if already up)
RBStart()
