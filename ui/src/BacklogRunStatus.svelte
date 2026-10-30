<script lang="ts">
  import type { BacklogView, ProtocolRun, GmailView } from './api'
  type Status = BacklogView['runs'][number]['status'] | ProtocolRun['status'] | GmailView['journal']['runs'][number]['status']
  let { status, lang }: { status: Status; lang: 'en' | 'de' } = $props()
  const text = {
    en: {
      prepared: { label: 'Prepared', detail: 'The saved plan is ready for review. Starting a new run requires explicit confirmation.' },
      running: { label: 'Running', detail: 'A batch may already have started. Cancel stops new batches and keeps completed changes.' },
      stopped: { label: 'Stopped', detail: 'New sorting batches are stopped. Completed changes stay recorded. Resume and Undo are separate actions.' },
      completed: { label: 'Processing complete', detail: 'Check the recorded results and conflicts. Completion does not mean every selected message changed or was restored.' },
      failed: { label: 'Failed', detail: 'Reload the saved journal and inspect recorded effects before continuing. Failure does not undo completed changes.' },
      uncertain: { label: 'Effects uncertain', detail: 'An attempted action has no conclusive observation. Do not assume it failed. Reconcile the saved effects using the available controls before continuing.' },
      complete: { label: 'Processing complete', detail: 'Check the recorded results and conflicts. Completion does not mean every selected message changed or was restored.' },
      conflict: { label: 'Conflicts found', detail: 'Safety checks found conflicts. Inspect the recorded effects and changed message identities before choosing another action.' },
      undo_running: { label: 'Undo in progress', detail: 'This run is processing separately confirmed Undo actions. It is not starting a new sorting run.' },
      undone: { label: 'Undo finished', detail: 'Check the recorded results and conflicts. Undo restores only eligible recorded effects and can leave messages unchanged.' },
    },
    de: {
      prepared: { label: 'Vorbereitet', detail: 'Der gespeicherte Plan kann geprüft werden. Der Start eines neuen Ablaufs braucht eine ausdrückliche Bestätigung.' },
      running: { label: 'Läuft', detail: 'Ein Stapel kann bereits gestartet sein. Abbrechen stoppt neue Stapel und behält abgeschlossene Änderungen.' },
      stopped: { label: 'Angehalten', detail: 'Neue Sortierstapel sind angehalten. Abgeschlossene Änderungen bleiben gespeichert. Fortsetzen und Rückgängigmachen sind separate Aktionen.' },
      completed: { label: 'Verarbeitung abgeschlossen', detail: 'Gespeicherte Ergebnisse und Konflikte prüfen. Der Abschluss bedeutet nicht, dass jede ausgewählte Nachricht geändert oder wiederhergestellt wurde.' },
      failed: { label: 'Fehlgeschlagen', detail: 'Gespeichertes Journal neu laden und aufgezeichnete Änderungen vor dem Fortsetzen prüfen. Ein Fehler macht abgeschlossene Änderungen nicht rückgängig.' },
      uncertain: { label: 'Änderungen ungewiss', detail: 'Für eine versuchte Aktion fehlt eine eindeutige Beobachtung. Nicht davon ausgehen, dass sie fehlgeschlagen ist. Gespeicherte Änderungen vor dem Fortsetzen mit den verfügbaren Bedienelementen abgleichen.' },
      complete: { label: 'Verarbeitung abgeschlossen', detail: 'Gespeicherte Ergebnisse und Konflikte prüfen. Der Abschluss bedeutet nicht, dass jede ausgewählte Nachricht geändert oder wiederhergestellt wurde.' },
      conflict: { label: 'Konflikte festgestellt', detail: 'Sicherheitsprüfungen haben Konflikte festgestellt. Aufgezeichnete Änderungen und geänderte Nachrichtenidentitäten vor einer weiteren Aktion prüfen.' },
      undo_running: { label: 'Rückgängigmachen läuft', detail: 'Dieser Ablauf verarbeitet separat bestätigte Rückgängig-Aktionen. Er startet keinen neuen Sortierlauf.' },
      undone: { label: 'Rückgängigmachen beendet', detail: 'Gespeicherte Ergebnisse und Konflikte prüfen. Nur geeignete aufgezeichnete Änderungen werden rückgängig gemacht. Nachrichten können unverändert bleiben.' },
    },
  } satisfies Record<'en' | 'de', Record<Status, { label: string; detail: string }>>
  let t = $derived(text[lang][status])
</script>
<p class="run-state" data-run-status={status}>{t.label}</p>
<p class="run-explanation">{t.detail}</p>
<style>
  .run-state { font-weight: 650; margin-block-end: .25rem; }
  .run-explanation { max-width: 72ch; margin-block-start: 0; }
  p { overflow-wrap: anywhere; }
</style>
