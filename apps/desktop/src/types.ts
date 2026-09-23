export interface RecordingInfo {
  name: string;
  path: string;
  created_at: string;
  size_bytes: number;
  transcript?: string | null;
}

export interface RecordingStartedPayload {
  path: string;
}

export interface RecordingStoppedPayload {
  path: string;
}

export interface AudioLevelPayload {
  rms: number;
  peak: number;
}

export interface ModelStatus {
  available: boolean;
  path: string;
}

export interface TranscriptionQueuedPayload {
  recording_path: string;
}

export interface TranscriptionStartedPayload {
  recording_path: string;
}

export interface TranscriptionCompletedPayload {
  recording_path: string;
  transcript: string;
}

export interface TranscriptionFailedPayload {
  recording_path: string;
  error: string;
}

export interface ModelDownloadProgressPayload {
  downloaded_bytes: number;
  total_bytes: number;
}

export interface ModelDownloadFailedPayload {
  error: string;
}

export type TranscriptionStatus = "queued" | "transcribing" | "done" | "error";

export interface RecordingErrorPayload {
  message: string;
}
