# Escolha da saída de som na gravação — design curto

## Intenção
Na aba Gravar, a pessoa já escolhe o microfone. Ela quer escolher também **qual saída de som** gravar como "som do computador": tem várias saídas, e a da reunião não é a padrão do sistema.

## Metas
- Seletor **Saída de som** nos modos "Som do computador" e "Microfone + som do computador", abaixo do seletor de microfone.
- Primeira opção = comportamento de hoje: **"Todas as saídas"** no Mac (tap global), **"Saída padrão"** no Windows (segue a troca da padrão). Depois, cada saída pelo nome; a padrão do sistema marcada "(padrão)".
- A escolha é lembrada (como o microfone). Saída salva que não existe mais → primeira opção, sem aviso.
- Saída escolhida desconectada durante a gravação → passa para a primeira opção e continua gravando, com um aviso discreto sob as barras: "A saída de som escolhida foi desconectada. Gravando agora de todas as saídas." (Mac) / "… da saída padrão." (Windows).

## Não-metas
- Voltar sozinho para a saída escolhida se ela reconectar (fica na primeira opção até o fim da gravação).
- Escolher processos/apps específicos.
- Mudar o jeito atual quando a primeira opção está escolhida (a pessoa confirmou que funciona).

## Restrições
- Mac: continua sendo o process tap do Core Audio (não a cpal, que grava o microfone de fones). Saída escolhida = `CATapDescription initExcludingProcesses:[] andDeviceUID:<UID> withStream:0`. O UID é o `DeviceId::id()` da cpal (o id em texto é `coreaudio:<UID>`).
- Windows: loopback WASAPI da cpal sobre a saída escolhida.
- Uma saída com várias streams: grava a stream 0 (decisão minha).
- Sem novas dependências.

## Abordagens
Só uma faz sentido: o tap por dispositivo da Apple no Mac e o loopback no dispositivo escolhido no Windows. A alternativa no Mac (cpal sobre a saída) foi descartada: com fones que também têm microfone ela grava o microfone. Escolhida pela pessoa: "sim, segue".

## Decisões (minhas, aprovadas na conversa)
- Queda da saída escolhida → cai para a primeira opção e continua, com aviso (numa reunião o som costuma migrar para outra saída; parar perderia o resto).
- Detecção da queda: a sessão confere a cada 1 s se a saída escolhida ainda está na lista (mesmo ritmo da checagem da saída padrão no Windows), além das falhas que a captura já reporta.

## Fica para quem constrói
Nomes internos, a forma exata do vínculo da captura com a saída, o layout fino do seletor.
