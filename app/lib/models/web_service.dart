import 'package:flutter/material.dart';

class WebService {
  final String   id;
  final String   name;
  final String   url;
  final IconData icon;
  final Color    color;
  final bool     isCustom; // добавлен овнером

  const WebService({
    required this.id,
    required this.name,
    required this.url,
    required this.icon,
    required this.color,
    this.isCustom = false,
  });

  WebService copyWith({
    String? name, String? url, IconData? icon, Color? color, bool? isCustom,
  }) => WebService(
    id:       id,
    name:     name     ?? this.name,
    url:      url      ?? this.url,
    icon:     icon     ?? this.icon,
    color:    color    ?? this.color,
    isCustom: isCustom ?? this.isCustom,
  );

  Map<String, dynamic> toJson() => {
    'id':       id,
    'name':     name,
    'url':      url,
    'iconCode': icon.codePoint,
    'color':    color.value,
    'isCustom': isCustom,
  };

  factory WebService.fromJson(Map<String, dynamic> j) => WebService(
    id:       j['id']   as String,
    name:     j['name'] as String,
    url:      j['url']  as String,
    icon:     IconData(j['iconCode'] as int, fontFamily: 'MaterialIcons'),
    color:    Color(j['color'] as int),
    isCustom: j['isCustom'] as bool? ?? false,
  );
}

/// Весь встроенный каталог. Tier'ов нет — тир определяется подпиской.
/// Telegram и Viber — первые два (рекомендуемые для базовой подписки).
const List<WebService> kBuiltinServices = [
  WebService(
    id: 'telegram', name: 'Telegram',
    url: 'https://web.telegram.org/',
    icon: Icons.send, color: Color(0xFF2AABEE),
  ),
  WebService(
    id: 'viber', name: 'Viber',
    url: 'https://web.viber.com/',
    icon: Icons.call, color: Color(0xFF7360F2),
  ),
  WebService(
    id: 'whatsapp', name: 'WhatsApp',
    url: 'https://web.whatsapp.com/',
    icon: Icons.chat, color: Color(0xFF25D366),
  ),
  WebService(
    id: 'instagram', name: 'Instagram',
    url: 'https://www.instagram.com/',
    icon: Icons.camera_alt, color: Color(0xFFE1306C),
  ),
  WebService(
    id: 'google', name: 'Google',
    url: 'https://www.google.com/',
    icon: Icons.search, color: Color(0xFF4285F4),
  ),
  WebService(
    id: 'twitter', name: 'X (Twitter)',
    url: 'https://x.com/',
    icon: Icons.alternate_email, color: Color(0xFF000000),
  ),
  WebService(
    id: 'youtube', name: 'YouTube',
    url: 'https://www.youtube.com/',
    icon: Icons.play_circle_fill, color: Color(0xFFFF0000),
  ),
  WebService(
    id: 'discord', name: 'Discord',
    url: 'https://discord.com/app',
    icon: Icons.headset_mic, color: Color(0xFF5865F2),
  ),
  WebService(
    id: 'reddit', name: 'Reddit',
    url: 'https://www.reddit.com/',
    icon: Icons.forum, color: Color(0xFFFF4500),
  ),
  WebService(
    id: 'linkedin', name: 'LinkedIn',
    url: 'https://www.linkedin.com/',
    icon: Icons.work, color: Color(0xFF0A66C2),
  ),
  WebService(
    id: 'facebook', name: 'Facebook',
    url: 'https://m.facebook.com/',
    icon: Icons.facebook, color: Color(0xFF1877F2),
  ),
  WebService(
    id: 'tiktok', name: 'TikTok',
    url: 'https://www.tiktok.com/',
    icon: Icons.music_note, color: Color(0xFF010101),
  ),
];
